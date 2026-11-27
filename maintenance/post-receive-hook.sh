#!/usr/bin/env bash
set -euo pipefail

DEPLOY_ROOT="/home/prajna/deploy/face-admin-next"
REPO_DIR="${DEPLOY_ROOT}/repo.git"
WORKTREE="${DEPLOY_ROOT}/worktree"
LOGS_DIR="${DEPLOY_ROOT}/logs"
ZERO_SHA="0000000000000000000000000000000000000000"

mkdir -p "$LOGS_DIR"

while read -r oldrev newrev refname; do
  if [ "$refname" != "refs/heads/main" ]; then
    echo "[post-receive] ignoring non-main ref: ${refname}"
    continue
  fi
  if [ "$newrev" = "$ZERO_SHA" ]; then
    echo "[post-receive] ignoring branch deletion on ${refname}"
    continue
  fi

  unset GIT_DIR GIT_WORK_TREE

  TS="$(date +%Y%m%d%H%M%S)"
  SHORT_SHA="${newrev:0:7}"
  LOG_FILE="${LOGS_DIR}/deploy-${TS}-${SHORT_SHA}.log"
  TMP_WT="$(mktemp -d "${DEPLOY_ROOT}/worktree-next.XXXXXX")"
  OLD_WT="${DEPLOY_ROOT}/worktree-prev.${TS}.$$"

  cleanup() {
    rm -rf "$TMP_WT" "$OLD_WT" 2>/dev/null || true
  }
  trap cleanup EXIT

  echo "[post-receive] exporting ${SHORT_SHA} (${refname})..."
  git --git-dir="$REPO_DIR" archive "$newrev" | tar -x -C "$TMP_WT"

  if [ -d "$WORKTREE" ]; then
    mv "$WORKTREE" "$OLD_WT"
  fi
  mv "$TMP_WT" "$WORKTREE"
  rm -rf "$OLD_WT"

  if [ ! -f "$WORKTREE/maintenance/auto_deploy.py" ]; then
    echo "[post-receive] error: missing maintenance/auto_deploy.py in ${SHORT_SHA}" >&2
    exit 1
  fi

  python3 "$WORKTREE/maintenance/auto_deploy.py" \
    --git-dir "$REPO_DIR" \
    --worktree "$WORKTREE" \
    --old-sha "$oldrev" \
    --new-sha "$newrev" \
    --deploy-root "$DEPLOY_ROOT" 2>&1 | tee -a "$LOG_FILE"
done
