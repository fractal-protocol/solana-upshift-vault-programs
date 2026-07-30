// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";

export const token_mint_mainnet = {
    wsol: "So11111111111111111111111111111111111111112",
    usdg: "2u1tszSeqZ3qBWF3uNGPFc8TzMk2tdiwknnRMWGWjGWH"
}

/**
 * Share offset used by the test vaults, matching the program's `EXTRA_SHARES`
 * default.
 *
 * `initialize` takes the offset per vault because the right value depends on
 * what a base unit of the deposit mint is worth: it must dominate a 1-unit
 * retained sliver (an absolute count), while `MIN_SUPPLY_MULTIPLE * offset` is
 * the minimum first deposit, whose cost is that count times the base-unit price.
 * Must be a power of ten within the program's permitted band.
 */
export const DEFAULT_SHARE_OFFSET = new anchor.BN(1_000_000);
