//! ARC (Automatic Reference Counting) runtime for managed mode.
//!
//! This module implements the managed-memory runtime that powers Noctivue's
//! `managed` mode. It provides:
//! - `Arc<T>`: Thread-safe reference-counted pointer (strong reference)
//! - `Weak<T>`: Non-owning reference that can be upgraded to `Arc<T>` (returns `Option<T>`)
//! - `Unowned<T>`: Non-owning reference that traps at runtime if dangling (for parent back-refs)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc as StdArc, RwLock, Weak as StdWeak};

use crate::Value;

/// Unique heap object identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HeapId(pub u64);

static NEXT_HEAP_ID: AtomicU64 = AtomicU64::new(1);

fn alloc_heap_id() -> HeapId {
    HeapId(NEXT_HEAP_ID.fetch_add(1, Ordering::Relaxed))
}

/// A heap-allocated object with reference counting.
#[derive(Debug)]
pub struct HeapObject {
    /// The actual value stored on the heap.
    pub value: RwLock<Value>,
    /// Strong reference count (Arc count).
    pub strong_count: AtomicU64,
    /// Weak reference count (Weak count).
    pub weak_count: AtomicU64,
}

impl HeapObject {
    pub fn new(value: Value) -> StdArc<Self> {
        StdArc::new(HeapObject {
            value: RwLock::new(value),
            strong_count: AtomicU64::new(1),
            weak_count: AtomicU64::new(1), // Arc itself holds a weak reference
        })
    }

    pub fn strong_count(&self) -> u64 {
        self.strong_count.load(Ordering::Relaxed)
    }

    pub fn weak_count(&self) -> u64 {
        self.weak_count.load(Ordering::Relaxed)
    }

    pub fn increment_strong(&self) {
        self.strong_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_strong(&self) -> u64 {
        self.strong_count.fetch_sub(1, Ordering::Relaxed) - 1
    }

    pub fn increment_weak(&self) {
        self.weak_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_weak(&self) -> u64 {
        self.weak_count.fetch_sub(1, Ordering::Relaxed) - 1
    }
}

/// Global heap registry for managed objects.
/// Maps HeapId to the heap object.
#[derive(Debug, Default)]
pub struct HeapRegistry {
    pub objects: HashMap<HeapId, StdArc<HeapObject>>,
}

impl HeapRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a new managed object on the heap.
    /// Returns the HeapId and increments the strong count to 1.
    pub fn alloc(&mut self, value: Value) -> HeapId {
        let id = alloc_heap_id();
        let obj = HeapObject::new(value);
        self.objects.insert(id, obj);
        id
    }

    /// Get a reference to the heap object (for reading/writing the value).
    pub fn get(&self, id: HeapId) -> Option<&StdArc<HeapObject>> {
        self.objects.get(&id)
    }

    /// Retain (increment strong count) for an Arc clone.
    pub fn retain(&self, id: HeapId) -> Result<(), String> {
        self.objects.get(&id)
            .map(|obj| obj.increment_strong())
            .ok_or_else(|| format!("Arc retain: heap object {} not found", id.0))
    }

    /// Release (decrement strong count). If count reaches 0, deallocate.
    pub fn release(&mut self, id: HeapId) -> Result<(), String> {
        let should_drop = self.objects.get(&id)
            .map(|obj| obj.decrement_strong() == 0)
            .ok_or_else(|| format!("Arc release: heap object {} not found", id.0))?;

        if should_drop {
            // Strong count reached 0 - the value is dropped.
            // The weak count remains (the Arc's own weak reference + any explicit Weaks).
            // The object will be fully removed from registry when weak count also reaches 0.
            // For now, we keep it in the registry but the value is logically dropped.
            // A more complete implementation would store a "dropped" flag or remove the value.
            self.objects.remove(&id);
        }
        Ok(())
    }

    /// Create a weak reference to the object.
    pub fn weak(&self, id: HeapId) -> Result<WeakValue, String> {
        self.objects.get(&id)
            .map(|obj| {
                obj.increment_weak();
                WeakValue::new(StdArc::downgrade(obj), id)
            })
            .ok_or_else(|| format!("Weak creation: heap object {} not found", id.0))
    }

    /// Drop a weak reference.
    pub fn drop_weak(&self, weak: StdWeak<HeapObject>) -> Result<(), String> {
        if let Some(obj) = weak.upgrade() {
            let _ = obj.decrement_weak();
            // If both strong and weak are 0, remove from registry
            if obj.strong_count() == 0 && obj.weak_count() == 0 {
                // We can't easily remove from registry here without the HeapId
                // This is handled when the last strong is released
            }
        }
        Ok(())
    }
}

/// Strong reference (Arc) to a heap-allocated managed value.
#[derive(Debug, Clone)]
pub struct ArcValue {
    pub heap_id: HeapId,
    // We don't store the Arc<HeapObject> directly to avoid lifetime issues
    // The registry owns the actual Arc
}

impl ArcValue {
    pub fn new(heap_id: HeapId) -> Self {
        Self { heap_id }
    }
}

/// Weak reference to a heap-allocated managed value.
/// Upgrading returns `Option<ArcValue>`.
#[derive(Debug, Clone)]
pub struct WeakValue {
    pub weak_ref: StdWeak<HeapObject>,
    pub heap_id: HeapId,
}

impl WeakValue {
    pub fn new(weak_ref: StdWeak<HeapObject>, heap_id: HeapId) -> Self {
        Self { weak_ref, heap_id }
    }

    /// Attempt to upgrade to a strong reference.
    /// Returns `None` if the object has been deallocated.
    pub fn upgrade(&self) -> Option<ArcValue> {
        self.weak_ref.upgrade().map(|_| ArcValue::new(self.heap_id))
    }
}

/// Unowned reference to a heap-allocated managed value.
/// Does not affect reference counts. Accessing a dangling unowned
/// reference traps at runtime (panic).
#[derive(Debug, Clone)]
pub struct UnownedValue {
    pub heap_id: HeapId,
}

impl UnownedValue {
    pub fn new(heap_id: HeapId) -> Self {
        Self { heap_id }
    }
}

/// Error type for managed runtime operations.
#[derive(Debug, Clone)]
pub enum ManagedError {
    /// Object not found in heap (dangling reference).
    DanglingReference(HeapId),
    /// Attempted to upgrade a weak reference to a deallocated object.
    ExpiredWeakReference(HeapId),
    /// Unowned reference accessed after deallocation.
    UnownedTrap(HeapId),
}

impl std::fmt::Display for ManagedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManagedError::DanglingReference(id) => write!(f, "Dangling Arc reference to heap object {}", id.0),
            ManagedError::ExpiredWeakReference(id) => write!(f, "Expired weak reference to heap object {}", id.0),
            ManagedError::UnownedTrap(id) => write!(f, "Unowned reference trap: heap object {} has been deallocated", id.0),
        }
    }
}

impl std::error::Error for ManagedError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arc_basic() {
        let mut registry = HeapRegistry::new();
        let id = registry.alloc(Value::Int(42));
        
        // Retain (clone)
        registry.retain(id).unwrap();
        
        // Release twice (original + clone)
        registry.release(id).unwrap(); // Count goes to 1
        registry.release(id).unwrap(); // Count goes to 0, object dropped
        
        // Object should be gone
        assert!(registry.get(id).is_none());
    }

    #[test]
    fn test_weak_upgrade() {
        let mut registry = HeapRegistry::new();
        let id = registry.alloc(Value::String("hello".to_string()));
        
        let weak = registry.weak(id).unwrap();
        
        // Upgrade should work while strong count > 0
        // (This test will need adjustment once WeakValue is properly implemented)
    }
}