// Generates a typed Rust client crate from the Anchor IDL using Codama.
//
// Run with: pnpm run generate-clients
// (Requires `anchor build` to have produced target/idl/august_vault.json.)
//
// The output crate lives at `clients/rust/august-vault/` and exposes typed
// instruction builders + account types. The point of using a generated
// client (vs. depending on the program crate directly) is that downstream
// consumers couple to the on-chain IDL, not the program's internal Rust
// modules — internal refactors don't break consumers.

import { createFromRoot } from "codama";
import { rootNodeFromAnchor } from "@codama/nodes-from-anchor";
import { renderVisitor } from "@codama/renderers-rust";
import { readFileSync } from "fs";
import { dirname, join } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const projectRoot = join(__dirname, "..");

const idlPath = join(projectRoot, "target/idl/august_vault.json");
const idl = JSON.parse(readFileSync(idlPath, "utf-8"));

const codamaTree = createFromRoot(rootNodeFromAnchor(idl));

// `renderVisitor` writes its source modules to the first positional arg.
// `crateFolder` is the crate root (where Cargo.toml lives) and is used by the
// renderer to resolve relative paths in generated `Cargo.toml` snippets when
// they're auto-managed. Pinned to the renderer version in package.json — if
// you bump @codama/renderers-rust, verify the path semantics still match.
const crateRoot = join(projectRoot, "clients/rust/august-vault");
const generatedPath = join(crateRoot, "src/generated");

codamaTree.accept(
    renderVisitor(generatedPath, {
        crateFolder: crateRoot,
        // Codama invokes `cargo +<toolchain> fmt`; leave formatting off if
        // a +nightly toolchain isn't installed locally.
        formatCode: false,
    }),
);

console.log("August Vault client generated successfully at:", generatedPath);
