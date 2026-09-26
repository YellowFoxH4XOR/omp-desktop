#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS bundle signing is only supported on macOS." >&2
  exit 1
fi

app_path="${1:-src-tauri/target/release/bundle/macos/πDesk.app}"

if [[ ! -d "$app_path" ]]; then
  echo "Application bundle not found: $app_path" >&2
  echo "Run 'bun run bundle:macos' first." >&2
  exit 1
fi

# Signature flags as a hex value (e.g. 0x10000 = hardened runtime, 0x2 = adhoc).
read_signature_flags() {
  local hex
  hex="$(codesign --display --verbose=4 "$1" 2>&1 |
    sed -n 's/.*flags=0x\([0-9a-fA-F]*\).*/\1/p' | head -1)"
  printf '%s' "${hex:-0}"
}

# Re-signing must never weaken the bundle's *policy* flags: whatever the
# previous signature enforced has to survive. Two bits are exempt because they
# describe the previous signature mechanism rather than a policy the app
# relies on: adhoc (0x2, codesign sets it for an ad-hoc identity) and
# linker-signed (0x20000, an artefact of the Mach-O having been linker-signed
# before codesign re-sealed it, which no re-sign can preserve).
RUNTIME_FLAG=$((0x10000))
MECHANISM_FLAGS=$((0x2 | 0x20000))
flags_before="$(read_signature_flags "$app_path")"
flags_before_dec=$((16#$flags_before))

# Ad-hoc sign for local use only (no Developer ID in this repo). Preserve the
# bundler's signature flags and entitlements instead of discarding them, and
# never use --deep: it applies the same settings to nested code and can mask
# per-component problems.
codesign --force --sign - --timestamp=none --preserve-metadata=flags,entitlements "$app_path"

# Verification: --strict surfaces structural problems, and any failure aborts
# the script (set -e) with a loud message.
if ! codesign --verify --strict --verbose=2 "$app_path"; then
  echo "codesign verification FAILED for: $app_path" >&2
  exit 1
fi

# Re-signing must never weaken the bundle: every flag the previous signature
# carried has to survive. (A missing hardened runtime is reported below, not
# treated as failure, because Tauri's local bundles are not built with one.)
flags_after="$(read_signature_flags "$app_path")"
flags_after_dec=$((16#$flags_after))
policy_before=$((flags_before_dec & ~MECHANISM_FLAGS))
policy_after=$((flags_after_dec & ~MECHANISM_FLAGS))
if (( (policy_after & policy_before) != policy_before )); then
  echo "Signature policy flags were weakened by re-signing: before=0x$flags_before after=0x$flags_after" >&2
  exit 1
fi

codesign --display --verbose=4 "$app_path"

if (( (flags_after_dec & RUNTIME_FLAG) == 0 )); then
  echo "Note: this bundle has no hardened runtime (ad-hoc local build)." >&2
  echo "Distributing it requires a Developer ID signature with '--options runtime', an entitlements file, and notarization." >&2
fi

echo "Ad-hoc signature verified for local use only. Gatekeeper will block distribution of ad-hoc-signed bundles; this is not notarized and must not be distributed."
