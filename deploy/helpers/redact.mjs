// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

/**
 * Render an RPC endpoint safely for logs.
 *
 * Provider endpoints routinely carry the API key in the query string or the
 * path (Helius uses `?api-key=`, others use `/<key>`), and these scripts print
 * the endpoint to identify the cluster — into terminals, CI logs and pasted
 * bug reports. Show enough to identify the cluster, never the credential.
 */
export function redactEndpoint(endpoint) {
  // NOT `String(endpoint)`: that yields the truthy string "undefined", which
  // defeats callers written as `redactEndpoint(x) || fallback` and prints
  // "Cluster actually used: undefined" right before a multi-SOL deploy.
  if (!endpoint) return '';
  try {
    const u = new URL(endpoint);
    const path = u.pathname && u.pathname !== '/' ? '/…' : '';
    return `${u.protocol}//${u.host}${path}${u.search ? '?…' : ''}`;
  } catch {
    return '<unparseable endpoint>';
  }
}
