// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! The queue's two account types. Layouts are pinned byte for byte in `tests.rs`,
//! as the vault's are, because both are created with `init` and cannot grow
//! without a migration once live.

pub mod withdrawal_queue;
pub mod withdrawal_request;

pub use withdrawal_queue::*;
pub use withdrawal_request::*;

#[cfg(test)]
mod tests;
