# reference-crud

Phase 5 reference application: CRUD REST API on SQLite, public
stdlib only.

Run it (from this directory, see `docs/overview.md` for the full
command):

    noct run ../../stdlib/core/convert.nv ../../stdlib/core/compare.nv \
      ../../stdlib/option/option.nv ../../stdlib/result/result.nv \
      ../../stdlib/strings/string.nv ../../stdlib/strings/format.nv \
      ../../stdlib/testing/assertions.nv ../../stdlib/net/http/request.nv \
      ../../stdlib/net/http/response.nv ../../stdlib/net/http/client.nv \
      ../../stdlib/net/http/server.nv ../../stdlib/db/sqlite.nv \
      ../../stdlib/db/pool.nv lib/main.nv

Run the serve smoke:

    noct test
