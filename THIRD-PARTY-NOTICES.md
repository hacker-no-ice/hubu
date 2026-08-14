# Third-Party Notices

Hubu binaries include third-party Rust crates recorded in `Cargo.lock`. Each
binary archive includes `THIRD-PARTY-LICENSES.txt`, generated from the locked,
target-specific normal dependency graph. That bundle identifies every included
crate and reproduces its distributed license, copyright, and notice files.

Hubu is built with the `rusqlite` `bundled` feature. The resulting binaries
include SQLite, which its authors have dedicated to the public domain. See the
[SQLite copyright notice](https://www.sqlite.org/copyright.html).

Some packages offer more than one license choice. `Cargo.lock` is the
authoritative version inventory for a particular Hubu source revision; the
generated bundle is the corresponding redistribution material for that
archive's target.

This notice is informational and does not alter the license terms of Hubu or
any third-party component.
