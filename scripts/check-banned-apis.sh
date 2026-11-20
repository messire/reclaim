#!/usr/bin/env bash
# Fails if product crates reference APIs Reclaim must never use (PRODUCT §4, ADR-0003).
# reclaim-bench is exempt: it is never shipped.
set -euo pipefail
cd "$(dirname "$0")/.."
pattern='TerminateProcess|NtTerminateProcess|NtSetSystemInformation|SetSystemFileCacheSize|SE_DEBUG_NAME|SeDebugPrivilege|QUOTA_LIMITS_HARDWS|SetProcessWorkingSetSizeEx'
if grep -rnE "$pattern" crates --include='*.rs' --include='*.toml' | grep -v '^crates/reclaim-bench/'; then
  echo "error: banned API referenced in a product crate (see ADR-0003)" >&2
  exit 1
fi
echo "banned API check: ok"
