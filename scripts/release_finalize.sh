#!/bin/bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/release_finalize.sh --repo-dir <path> --tap-dir <path> --tap-repo <owner/repo> \
    --formula-path <path> --github-repo <owner/repo> --prev-tag <tag-or-empty> \
    --new-version <version> --tag <tag> --tarball-path <path> --tarball-name <name> \
    --sha256 <sha256> --release-commit-sha <sha>
EOF
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

build_release_notes() {
  local prev_tag="$1"
  local new_tag="$2"
  local target_ref="$3"
  local github_repo="$4"
  local out_file="$5"
  local compare_url="https://github.com/$github_repo/compare"
  local changes

  if [ -n "$prev_tag" ]; then
    changes="$(git log --no-merges --pretty=format:'%s' "$prev_tag..$target_ref" \
      | grep -vE '^bump version to ' \
      | head -n 30 || true)"
  else
    changes="$(git log --no-merges --pretty=format:'%s' "$target_ref" \
      | grep -vE '^bump version to ' \
      | head -n 30 || true)"
  fi

  {
    echo "MicroClaw $new_tag"
    echo
    echo "## Change log"
    if [ -n "$changes" ]; then
      while IFS= read -r line; do
        [ -n "$line" ] && echo "- $line"
      done <<< "$changes"
    else
      echo "- Internal maintenance and release packaging updates"
    fi
    echo
    echo "## Compare"
    if [ -n "$prev_tag" ]; then
      echo "$compare_url/$prev_tag...$new_tag"
    else
      echo "N/A (first tagged release)"
    fi
  } > "$out_file"
}

wait_for_ci_success() {
  local github_repo="$1"
  local commit_sha="$2"
  local timeout_seconds="${CI_WAIT_TIMEOUT_SECONDS:-1800}"
  local interval_seconds="${CI_WAIT_INTERVAL_SECONDS:-20}"
  local elapsed=0

  echo "Waiting for CI success on commit: $commit_sha"
  while [ "$elapsed" -lt "$timeout_seconds" ]; do
    local success_run_id
    success_run_id="$(
      gh run list \
        --repo "$github_repo" \
        --workflow "CI" \
        --commit "$commit_sha" \
        --json databaseId,conclusion \
        --jq '[.[] | select(.conclusion == "success")] | first | .databaseId'
    )"

    if [ -n "$success_run_id" ] && [ "$success_run_id" != "null" ]; then
      echo "CI succeeded. Run id: $success_run_id"
      return 0
    fi

    local failed_run_url
    failed_run_url="$(
      gh run list \
        --repo "$github_repo" \
        --workflow "CI" \
        --commit "$commit_sha" \
        --json conclusion,url \
        --jq '[.[] | select(.conclusion == "failure" or .conclusion == "cancelled" or .conclusion == "timed_out" or .conclusion == "action_required" or .conclusion == "startup_failure" or .conclusion == "stale")] | first | .url'
    )"

    if [ -n "$failed_run_url" ] && [ "$failed_run_url" != "null" ]; then
      echo "CI failed for commit $commit_sha: $failed_run_url" >&2
      return 1
    fi

    echo "CI not successful yet. Slept ${elapsed}s/${timeout_seconds}s."
    sleep "$interval_seconds"
    elapsed=$((elapsed + interval_seconds))
  done

  echo "Timed out waiting for CI success after ${timeout_seconds}s." >&2
  return 1
}

REPO_DIR=""
TAP_DIR=""
TAP_REPO=""
FORMULA_PATH=""
GITHUB_REPO=""
PREV_TAG=""
NEW_VERSION=""
TAG=""
TARBALL_PATH=""
TARBALL_NAME=""
SHA256=""
RELEASE_COMMIT_SHA=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo-dir) REPO_DIR="$2"; shift 2 ;;
    --tap-dir) TAP_DIR="$2"; shift 2 ;;
    --tap-repo) TAP_REPO="$2"; shift 2 ;;
    --formula-path) FORMULA_PATH="$2"; shift 2 ;;
    --github-repo) GITHUB_REPO="$2"; shift 2 ;;
    --prev-tag) PREV_TAG="$2"; shift 2 ;;
    --new-version) NEW_VERSION="$2"; shift 2 ;;
    --tag) TAG="$2"; shift 2 ;;
    --tarball-path) TARBALL_PATH="$2"; shift 2 ;;
    --tarball-name) TARBALL_NAME="$2"; shift 2 ;;
    --sha256) SHA256="$2"; shift 2 ;;
    --release-commit-sha) RELEASE_COMMIT_SHA="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

for required in REPO_DIR TAP_DIR TAP_REPO FORMULA_PATH GITHUB_REPO NEW_VERSION TAG TARBALL_PATH TARBALL_NAME SHA256 RELEASE_COMMIT_SHA; do
  if [ -z "${!required}" ]; then
    echo "Missing required argument: $required" >&2
    usage >&2
    exit 1
  fi
done

require_cmd gh
require_cmd git
require_cmd shasum

if ! gh auth status >/dev/null 2>&1; then
  echo "GitHub CLI not authenticated. Run: gh auth login" >&2
  exit 1
fi

cd "$REPO_DIR"

if ! wait_for_ci_success "$GITHUB_REPO" "$RELEASE_COMMIT_SHA"; then
  exit 1
fi

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
  echo "Tag already exists locally: $TAG"
else
  git tag "$TAG" "$RELEASE_COMMIT_SHA"
  echo "Created tag: $TAG -> $RELEASE_COMMIT_SHA"
fi

git push origin "refs/tags/$TAG"
echo "Pushed tag: $TAG"

RELEASE_NOTES_FILE="target/release/release-notes-$TAG.md"
build_release_notes "$PREV_TAG" "$TAG" "$RELEASE_COMMIT_SHA" "$GITHUB_REPO" "$RELEASE_NOTES_FILE"

if gh release view "$TAG" --repo "$GITHUB_REPO" >/dev/null 2>&1; then
  echo "Release $TAG already exists. Uploading/overwriting asset."
  gh release upload "$TAG" "$TARBALL_PATH" --repo "$GITHUB_REPO" --clobber
else
  gh release create "$TAG" "$TARBALL_PATH" \
    --repo "$GITHUB_REPO" \
    -t "$TAG" \
    -F "$RELEASE_NOTES_FILE"
  echo "Created GitHub release: $TAG"
fi

if [ ! -d "$TAP_DIR/.git" ]; then
  echo "Cloning tap repo..."
  git clone "https://github.com/$TAP_REPO.git" "$TAP_DIR"
fi

cd "$TAP_DIR"
mkdir -p Formula

cat > "$FORMULA_PATH" << RUBY
class Microclaw < Formula
  desc "Agentic AI assistant for Telegram - web search, scheduling, memory, tool execution"
  homepage "https://github.com/$GITHUB_REPO"
  url "https://github.com/$GITHUB_REPO/releases/download/$TAG/$TARBALL_NAME"
  sha256 "$SHA256"
  license "MIT"

  def install
    bin.install "microclaw"
  end

  test do
    assert_match "MicroClaw", shell_output("#{bin}/microclaw help")
  end
end
RUBY

git add .
git commit -m "microclaw homebrew release $NEW_VERSION"
git push

echo ""
echo "Done! Released $TAG and updated Homebrew tap."
echo ""
echo "Users can install with:"
echo "  brew tap everettjf/tap"
echo "  brew install microclaw"
