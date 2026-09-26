-- Noctivue Neovim support (minimal, second-editor proof).
--
-- Reuses the existing `noctivue-lsp` binary over stdio: no new protocol,
-- no plugins required (built-in LSP client, Neovim 0.8+).
--
-- Install: copy/symlink this file into your Neovim config (e.g. as
-- `~/.config/nvim/lua/noctivue.lua`) and add `require('noctivue')` to
-- your `init.lua`.
--
-- Server resolution mirrors editors/vscode/src/extension.ts:
--   * `vim.g.noctivue_lsp_server_path` overrides everything
--     (same role as VS Code's `noctivue.lsp.serverPath`).
--   * Otherwise `<workspace-root>/target/release/noctivue-lsp[.exe]`,
--     falling back to `<workspace-root>/target/debug/...`.
--     Release is preferred; a stale debug binary must never shadow it.
--   * `vim.g.noctivue_lsp_enabled = false` disables autostart
--     (same role as VS Code's `noctivue.lsp.enabled`, default true).

-- 1. Filetype detection for *.nv + nestpkg. VS Code contributes
-- language ids `noctivue` (extension `.nv`) and `nestpkg` (extensions
-- `.nvpm`, fixed basenames); keep the same names here. Only the fixed
-- `nestpkg.lock` basename is claimed — never generic `*.lock`.
vim.filetype.add({
  extension = {
    nv = 'noctivue',
    nvpm = 'nestpkg',
  },
  filename = {
    ['nestpkg.nvpm'] = 'nestpkg',
    ['nestpkg.lock'] = 'nestpkg',
  },
})

local function binary_name()
  -- Mirrors `process.platform === 'win32'` in extension.ts.
  if vim.fn.has('win32') == 1 then
    return 'noctivue-lsp.exe'
  end
  return 'noctivue-lsp'
end

local function start_dir(bufname)
  if bufname ~= '' then
    return vim.fs.dirname(bufname)
  end
  return vim.fn.getcwd()
end

-- Outermost ancestor containing Cargo.toml, i.e. the cargo workspace root
-- whose `target/` holds the server binary (mirrors extension.ts resolving
-- the project root above editors/vscode).
local function workspace_root(dir)
  local root = nil
  for parent in vim.fs.parents(dir) do
    if vim.fn.filereadable(parent .. '/Cargo.toml') == 1 then
      root = parent
    end
  end
  return root or dir
end

local function resolve_server(root)
  local override = vim.g.noctivue_lsp_server_path
  if override ~= nil and override ~= '' then
    return override
  end
  local release = root .. '/target/release/' .. binary_name()
  if vim.fn.executable(release) == 1 then
    return release
  end
  return root .. '/target/debug/' .. binary_name()
end

local group = vim.api.nvim_create_augroup('NoctivueLsp', { clear = true })

vim.api.nvim_create_autocmd('FileType', {
  group = group,
  -- One client serves both filetypes (same single `noctivue-lsp`
  -- binary the VS Code extension uses; the server routes by filename).
  pattern = { 'noctivue', 'nestpkg' },
  callback = function(args)
    -- Same default as `noctivue.lsp.enabled` in package.json (true).
    if vim.g.noctivue_lsp_enabled == false then
      return
    end
    local bufname = vim.api.nvim_buf_get_name(args.buf)
    local root = workspace_root(start_dir(bufname))
    local server = resolve_server(root)
    if vim.fn.executable(server) ~= 1 then
      vim.notify(
        'Noctivue LSP server not found at: ' .. server
          .. ". Build it with 'cargo build --release -p noctivue-lsp'"
          .. ' or set vim.g.noctivue_lsp_server_path.',
        vim.log.levels.ERROR
      )
      return
    end
    -- stdio transport, no extra args: mirrors extension.ts ServerOptions
    -- (TransportKind.stdio) and noctivue-lsp's Connection::stdio().
    -- vim.lsp.start reuses the client for the same root_dir, so
    -- multi-root sessions get one client per workspace. No new protocol.
    vim.lsp.start({
      name = 'noctivue',
      cmd = { server },
      root_dir = root,
    })
  end,
})
