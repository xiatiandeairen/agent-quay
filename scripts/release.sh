#!/bin/bash
# 发版：默认只预览（版本号、变更说明、要做的步骤），不改任何东西；确认后加 --yes 执行。
#   scripts/release.sh [版本号] [--notes 文件] [--yes | --local]
# 版本号不写时按上一个 tag 以来的提交推算：正文有 "BREAKING CHANGE:" 升次版本号，否则升修订号。
# --local：改版本号、提交、打 tag、构建安装包，不推送、不发 GitHub Release、不改 tap（用来验证）。
# --yes：再推送 main 和 tag、发 GitHub Release、更新 tap 公式。tap 检出位置默认是仓库旁边的 ../homebrew-tap，可用 AQ_TAP_DIR 改。
# 直接运行，不要经 aq run：just check 里的 nice 测试在 aq 下必然失败（见 AGENTS.md 的坑）。
set -euo pipefail
cd "$(dirname "$0")/.."

die() { echo "release: $*" >&2; exit 1; }
want="" notes_file="" mode=preview
while [ $# -gt 0 ]; do
  case "$1" in
    --yes) mode=publish ;;
    --local) mode=local ;;
    --notes) notes_file="${2:-}"; shift ;;
    -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
    -*) die "unknown option $1" ;;
    *) want="$1" ;;
  esac
  shift
done
tap_dir="${AQ_TAP_DIR:-../homebrew-tap}"

# ---------- 现状 ----------
[ "$(git branch --show-current)" = main ] || die "not on main"
git fetch -q origin
[ -z "$(git rev-list HEAD..origin/main)" ] || die "main is behind origin/main; pull first"
last=$(git describe --tags --abbrev=0 --match 'v[0-9]*')
cur=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml)
[ "v${cur}" = "${last}" ] || die "Cargo.toml says ${cur} but the last tag is ${last}"
range="${last}..HEAD"
[ -n "$(git rev-list "${range}")" ] || die "no commits since ${last}"

IFS=. read -r major minor patch <<EOF
${cur}
EOF
if git log "${range}" --format=%B | grep -q '^BREAKING CHANGE:'; then
  next="${major}.$((minor + 1)).0" why="has BREAKING CHANGE commits"
else
  next="${major}.${minor}.$((patch + 1))" why="no BREAKING CHANGE commits"
fi
if [ -n "${want}" ]; then
  echo "${want}" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "version must look like 1.2.3"
  [ "$(printf '%s\n%s\n' "${cur}" "${want}" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)" = "${want}" ] && [ "${want}" != "${cur}" ] \
    || die "${want} is not newer than ${cur}"
  why="given on the command line (computed would be ${next})"
  next="${want}"
fi

# ---------- 变更说明 ----------
dist="target/dist"
mkdir -p "${dist}"
notes="${dist}/notes-${next}.md"
if [ -n "${notes_file}" ]; then
  cp "${notes_file}" "${notes}"
else
  {
    section() { # 标题 提交类型
      local lines
      lines=$(git log "${range}" --reverse --format=%s | sed -n "s/^$2: /- /p")
      [ -z "${lines}" ] || printf '### %s\n%s\n\n' "$1" "${lines}"
    }
    breaking=$(git log "${range}" --reverse --format=%B | sed -n 's/^BREAKING CHANGE: */- /p')
    [ -z "${breaking}" ] || printf '### Breaking\n%s\n\n' "${breaking}"
    section Added feat
    section Fixed fix
    section Performance perf
  } > "${notes}"
fi

echo "aq ${cur} -> ${next} (${why}); $(git rev-list --count "${range}") commits since ${last}"
echo "----- release notes (${notes}) -----"
cat "${notes}"
echo "-----"
echo "--local: just check, bump 7 version fields, commit, tag v${next}, build universal ${dist}/aq-${next}-macos.tar.gz"
echo "--yes:   all of the above, then push main and v${next}, create the GitHub release, update ${tap_dir}/Formula/aq.rb"
[ "${mode}" != preview ] || { echo "preview only; nothing changed. Edit the notes with --notes <file> if needed."; exit 0; }
[ -z "$(git status --porcelain)" ] || die "working tree is not clean; commit or stash first"

# ---------- 改版本号、提交、打 tag ----------
just check
sed -i '' -e "/^\[workspace.package\]/,/^\[/s/^version = \"${cur}\"$/version = \"${next}\"/" \
  -e "s/version = \"${cur}\", path/version = \"${next}\", path/" Cargo.toml crates/*/Cargo.toml
sed -i '' "s/\"version\": \"${cur}\"/\"version\": \"${next}\"/" .claude-plugin/plugin.json
cargo update --workspace --offline -q
[ "$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml)" = "${next}" ] || die "workspace version not bumped"
grep -q "\"version\": \"${next}\"" .claude-plugin/plugin.json || die "plugin.json not bumped"
if grep -n "version = \"${cur}\", path" Cargo.toml crates/*/Cargo.toml; then die "path dependencies still at ${cur}"; fi
git add Cargo.toml Cargo.lock crates/*/Cargo.toml .claude-plugin/plugin.json
git commit -q -m "chore: release ${next}"
git tag -a "v${next}" -m "aq ${next}"

# ---------- 构建通用二进制 ----------
for t in aarch64-apple-darwin x86_64-apple-darwin; do cargo build -q --release --locked --target "${t}"; done
lipo -create -output "${dist}/aq" target/aarch64-apple-darwin/release/aq target/x86_64-apple-darwin/release/aq
[ "$(lipo -archs "${dist}/aq")" = "x86_64 arm64" ] || die "binary is not universal: $(lipo -archs "${dist}/aq")"
[ "$("${dist}/aq" --version)" = "aq ${next}" ] || die "built binary reports $("${dist}/aq" --version)"
tarball="${dist}/aq-${next}-macos.tar.gz"
tar -czf "${tarball}" -C "${dist}" aq
sha=$(shasum -a 256 "${tarball}" | cut -d' ' -f1)
echo "built ${tarball} sha256 ${sha}"
[ "${mode}" = publish ] || { echo "--local done: commit and tag v${next} are local only."; exit 0; }

# ---------- 发布 ----------
repo=$(gh repo view --json nameWithOwner -q .nameWithOwner)
git push -q origin refs/heads/main:refs/heads/main "refs/tags/v${next}:refs/tags/v${next}"
gh release create "v${next}" "${tarball}" --repo "${repo}" --title "aq ${next}" --notes-file "${notes}"
[ "$(curl -sfL "https://github.com/${repo}/releases/download/v${next}/aq-${next}-macos.tar.gz" | shasum -a 256 | cut -d' ' -f1)" = "${sha}" ] \
  || die "uploaded asset sha256 does not match"

[ -z "$(git -C "${tap_dir}" status --porcelain)" ] || die "${tap_dir} is not clean"
git -C "${tap_dir}" pull -q --ff-only
formula="${tap_dir}/Formula/aq.rb"
sed -i '' -e "s|releases/download/v[0-9.]*/aq-[0-9.]*-macos.tar.gz|releases/download/v${next}/aq-${next}-macos.tar.gz|" \
  -e "s|sha256 \"[0-9a-f]*\"|sha256 \"${sha}\"|" "${formula}"
grep -q "v${next}/aq-${next}" "${formula}" && grep -q "${sha}" "${formula}" || die "failed to update ${formula}"
git -C "${tap_dir}" commit -q -am "feat: update aq to ${next}"
git -C "${tap_dir}" push -q origin HEAD
echo "released aq ${next}: https://github.com/${repo}/releases/tag/v${next}; brew upgrade aq to install"
