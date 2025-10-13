#!/usr/bin/env bash

set -euo pipefail

DEFAULT_CACHE_ROOT="tmp/snapshots/_overlay_cache"
CACHE_ROOT="${AARDVARK_OVERLAY_CACHE_DIR:-$DEFAULT_CACHE_ROOT}"

print_usage() {
  cat <<'EOF'
overlay-cache-sync.sh - sync the content-addressed overlay cache between hosts.

Usage:
  overlay-cache-sync.sh push <target_dir>
  overlay-cache-sync.sh pull <source_dir>
  overlay-cache-sync.sh push-s3 <bucket[/prefix]> [--profile PROFILE] [--region REGION]
  overlay-cache-sync.sh pull-s3 <bucket[/prefix]> [--profile PROFILE] [--region REGION]
  overlay-cache-sync.sh status

Environment:
  AARDVARK_OVERLAY_CACHE_DIR  Override cache root (default: tmp/snapshots/_overlay_cache)

Commands:
  push       Mirror the local cache into <target_dir> via rsync (adds --delete).
  pull       Mirror <source_dir> into the local cache via rsync (adds --delete).
  push-s3    Upload cache to an S3 bucket/prefix via aws-cli (requires aws CLI).
  pull-s3    Download cache from an S3 bucket/prefix via aws-cli (requires aws CLI).
  status     Print basic cache stats (blob count + total bytes).

Examples:
  # Push local cache to a shared NFS mount
  overlay-cache-sync.sh push /srv/shared/_overlay_cache

  # Pull cache from another machine
  overlay-cache-sync.sh pull user@host:/srv/shared/_overlay_cache

  # Sync to S3 (requires AWS credentials)
  overlay-cache-sync.sh push-s3 my-overlay-bucket/cache-prefix --profile prod
EOF
}

require_dir() {
  local dir="$1"
  if [[ ! -d "$dir" ]]; then
    mkdir -p "$dir"
  fi
}

check_rsync() {
  if ! command -v rsync >/dev/null 2>&1; then
    echo "error: rsync is required for this command" >&2
    exit 1
  fi
}

check_aws() {
  if ! command -v aws >/dev/null 2>&1; then
    echo "error: aws CLI is required for this command" >&2
    exit 1
  fi
}

cache_stats() {
  if [[ ! -d "$CACHE_ROOT" ]]; then
    echo "Cache directory '$CACHE_ROOT' does not exist."
    exit 0
  fi
  local count size
  count=$(find "$CACHE_ROOT" -type f -name 'sha256-*.tar' | wc -l | tr -d ' ')
  size=$(du -sh "$CACHE_ROOT" 2>/dev/null | awk '{print $1}')
  echo "Cache root: $CACHE_ROOT"
  echo "Blob files: $count"
  echo "Total size: ${size:-0}"
}

if [[ $# -lt 1 ]]; then
  print_usage
  exit 1
fi

COMMAND="$1"
shift

case "$COMMAND" in
  push)
    if [[ $# -ne 1 ]]; then
      echo "error: push requires <target_dir>" >&2
      print_usage
      exit 1
    fi
    check_rsync
    require_dir "$CACHE_ROOT"
    target="$1"
    rsync -av --delete "$CACHE_ROOT/" "$target/"
    ;;
  pull)
    if [[ $# -ne 1 ]]; then
      echo "error: pull requires <source_dir>" >&2
      print_usage
      exit 1
    fi
    check_rsync
    require_dir "$CACHE_ROOT"
    source_dir="$1"
    rsync -av --delete "$source_dir/" "$CACHE_ROOT/"
    ;;
  push-s3)
    if [[ $# -lt 1 ]]; then
      echo "error: push-s3 requires <bucket[/prefix]>" >&2
      print_usage
      exit 1
    fi
    check_aws
    require_dir "$CACHE_ROOT"
    bucket="$1"
    shift
    aws s3 sync "$CACHE_ROOT/" "s3://$bucket/" "$@" --delete
    ;;
  pull-s3)
    if [[ $# -lt 1 ]]; then
      echo "error: pull-s3 requires <bucket[/prefix]>" >&2
      print_usage
      exit 1
    fi
    check_aws
    require_dir "$CACHE_ROOT"
    bucket="$1"
    shift
    aws s3 sync "s3://$bucket/" "$CACHE_ROOT/" "$@" --delete
    ;;
  status)
    cache_stats
    ;;
  --help|-h|help)
    print_usage
    ;;
  *)
    echo "error: unknown command '$COMMAND'" >&2
    print_usage
    exit 1
    ;;
esac
