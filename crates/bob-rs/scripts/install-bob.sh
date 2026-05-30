#!/usr/bin/env bash
# Installs (or repairs) the Bob CLI + its Node.js dependency without
# requiring sudo. Driven by `POST /api/bob/install`; output streams
# back to the browser via SSE.
#
# Steps:
#   1. Ensure nvm (https://github.com/nvm-sh/nvm) is installed — it
#      lives in $HOME/.nvm, no root access required.
#   2. Ensure Node.js $REQUIRED_NODE_MAJOR is installed via nvm and
#      aliased to default.
#   3. Run the official IBM Bob installer in `--pm npm` non-
#      interactive mode.
#   4. Print `bob --version` so the caller can verify.
#
# All output is plain text — the server tags each line with an SSE
# event type before forwarding to the browser. Lines starting with
# `[BOB-INSTALL]` are status markers (used for browser progress
# checkpoints); everything else is normal stdout/stderr.

set -euo pipefail

REQUIRED_NODE_MAJOR="${REQUIRED_NODE_MAJOR:-22}"
NVM_VERSION="${NVM_VERSION:-v0.40.1}"

log_step() {
  printf '[BOB-INSTALL] %s\n' "$1"
}

# -----------------------------------------------------------------
# 1. nvm
# -----------------------------------------------------------------
export NVM_DIR="${NVM_DIR:-$HOME/.nvm}"

if [ -s "$NVM_DIR/nvm.sh" ]; then
  log_step "nvm: already installed at $NVM_DIR"
else
  log_step "nvm: installing $NVM_VERSION into $NVM_DIR"
  # nvm's install.sh sources its own shell init that we don't want
  # to touch when running under bash -c, so pipe to bash with the
  # PROFILE empty trick to skip the rc-file injection.
  curl -fsSL "https://raw.githubusercontent.com/nvm-sh/nvm/${NVM_VERSION}/install.sh" \
    | PROFILE=/dev/null bash
fi

# Source nvm so `nvm`, `node`, `npm` become callable in this shell.
# shellcheck disable=SC1091
. "$NVM_DIR/nvm.sh"

# -----------------------------------------------------------------
# 2. Node.js (correct major version)
# -----------------------------------------------------------------
if command -v node >/dev/null 2>&1 \
    && [ "$(node -p 'process.versions.node.split(".")[0]')" -ge "$REQUIRED_NODE_MAJOR" ]; then
  log_step "node: already on $(node --version) (≥ v${REQUIRED_NODE_MAJOR})"
else
  log_step "node: installing Node.js ${REQUIRED_NODE_MAJOR}.x via nvm"
  nvm install "$REQUIRED_NODE_MAJOR"
  nvm alias default "$REQUIRED_NODE_MAJOR"
fi
nvm use --silent default

# -----------------------------------------------------------------
# 3. bob CLI
# -----------------------------------------------------------------
if command -v bob >/dev/null 2>&1; then
  log_step "bob: already installed at $(command -v bob) ($(bob --version 2>/dev/null || echo unknown))"
  log_step "bob: re-running installer to update to latest"
fi

log_step "bob: downloading IBM installer"
# `--pm npm` keeps the install non-interactive — the script would
# otherwise prompt for a package manager and hang here.
curl -fsSL https://bob.ibm.com/download/bobshell.sh | bash -s -- --pm npm

# -----------------------------------------------------------------
# 4. Verify
# -----------------------------------------------------------------
log_step "bob: verifying"
BOB_PATH="$(command -v bob || true)"
BOB_VERSION="$(bob --version 2>/dev/null || echo 'verification failed')"
log_step "bob: path=${BOB_PATH:-<missing>} version=${BOB_VERSION}"
log_step "done"
