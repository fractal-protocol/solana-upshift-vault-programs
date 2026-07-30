#!/usr/bin/env bash
# Point git at the repo's tracked hooks. Run once per clone.
set -euo pipefail
cd "$(dirname "$0")/.."
git config core.hooksPath scripts/hooks
echo "git hooks installed (core.hooksPath = scripts/hooks)"
echo "installed:"
ls -1 scripts/hooks | sed 's/^/  /'
