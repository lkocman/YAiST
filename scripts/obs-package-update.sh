#!/bin/bash
# Updates the yaist package on src.opensuse.org to a snapshot of upstream
# main and opens a pull request for it, or refreshes the open one. Run by
# .github/workflows/obs-package.yml after every merge to main; works the same
# locally (needs git-lfs, python3, obs-service-obs_scm,
# obs-service-set_version, obs-service-cargo and cargo).
#
# The branch goes to a fork owned by OBS_BOT_USER, whose token only needs to
# write to its own repositories: the bot proposes, a maintainer merges.
#
#   OBS_PACKAGE_REPO  package repository (default: lkocman/YAiST)
#   OBS_PACKAGE_NAME  package (spec file) name (default: yaist)
#   OBS_BOT_USER      account with a fork of it (not needed with DRY_RUN)
#   OBS_BOT_TOKEN     that account's src.opensuse.org token
#   DRY_RUN=1         prepare the commit and show it; no push, no pull request
set -euo pipefail

GITEA=https://src.opensuse.org
PKG_REPO="${OBS_PACKAGE_REPO:-lkocman/YAiST}"
PKG_NAME="${OBS_PACKAGE_NAME:-yaist}"
BRANCH=yaist-update
if [ -z "${DRY_RUN:-}" ]; then
    : "${OBS_BOT_USER:?set OBS_BOT_USER}" "${OBS_BOT_TOKEN:?set OBS_BOT_TOKEN (or DRY_RUN=1)}"
fi
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# Large files stay LFS pointers: the snapshot and vendor tarball are
# regenerated anyway, nothing needs the old ones. Services such as
# cargo_vendor find the spec by the directory's name, as in an osc checkout.
GIT_LFS_SKIP_SMUDGE=1 git clone -q "$GITEA/$PKG_REPO.git" "$WORK/$PKG_NAME"
cd "$WORK/$PKG_NAME"
git lfs install --local >/dev/null
git switch -q -c "$BRANCH"

# Nothing to do when the package already has upstream's latest commit (a
# rerun); the vendor tarball would differ in bytes only.
UPSTREAM=$(sed -n 's|.*<param name="url">\(.*\)</param>.*|\1|p' _service | head -1)
LATEST=$(git ls-remote "$UPSTREAM" refs/heads/main | cut -f1)
if [ "$LATEST" = "$(sed -n 's/^commit: //p' "$PKG_NAME.obsinfo")" ]; then
    echo "The package is up to date (${LATEST:0:12})."
    exit 0
fi

# Run the manual services from _service, as `osc service manualrun` does:
# each writes to its own directory and its files replace the package's. A
# new snapshot replaces the old .obscpio. From cargo_vendor only the vendor
# tarball is kept: it also writes a source tarball and rewrites the spec's
# Version in its own format.
python3 - <<'EOF'
import glob, os, shutil, subprocess, tempfile
import xml.etree.ElementTree as ET

for svc in ET.parse("_service").getroot().iter("service"):
    if svc.get("mode") != "manual":
        continue
    name = svc.get("name")
    out = tempfile.mkdtemp()
    cmd = [f"/usr/lib/obs/service/{name}"]
    for p in svc.iter("param"):
        cmd += [f"--{p.get('name')}", p.text or ""]
    print("running", name, flush=True)
    specs = {s: open(s).read() for s in glob.glob("*.spec")}
    subprocess.run(cmd + ["--outdir", out], check=True)
    if name == "cargo_vendor":
        # It edits the spec in place, not in its output directory.
        for spec, text in specs.items():
            open(spec, "w").write(text)
    for f in os.listdir(out):
        if name == "cargo_vendor" and not f.startswith("vendor"):
            continue
        if f.endswith(".obscpio"):
            for old in glob.glob("*.obscpio"):
                os.remove(old)
        shutil.copy2(os.path.join(out, f), f)
    shutil.rmtree(out)
EOF

if [ -z "$(git status --porcelain)" ]; then
    echo "The package is up to date."
    exit 0
fi
VERSION=$(sed -n 's/^version: //p' "$PKG_NAME.obsinfo")
COMMIT=$(sed -n 's/^commit: //p' "$PKG_NAME.obsinfo")
git add -A
git -c user.name="${GIT_AUTHOR_NAME:-YAiST package bot}" -c user.email="${GIT_AUTHOR_EMAIL:-noreply@opensuse.org}" \
    commit -q -m "Update to $VERSION" -m "Snapshot of https://github.com/lkocman/YAiST/commit/$COMMIT"
git show --stat --format='%s%n%n%b' HEAD
# The new .changes entry, for the pull request.
CHANGES=$(awk 'NR > 1 && /^-{67}$/ { exit } { print }' "$PKG_NAME.changes")

if [ -n "${DRY_RUN:-}" ]; then
    echo "$CHANGES"
    echo "DRY_RUN: not pushing to $GITEA/${OBS_BOT_USER:-<bot>}/${PKG_REPO#*/}, no pull request."
    exit 0
fi

AUTH=$(printf '%s:%s' "$OBS_BOT_USER" "$OBS_BOT_TOKEN" | base64 -w0)
if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::add-mask::$AUTH"
fi
# The token goes in a header for this one push (git-lfs uses it too), never
# into a remote URL or .git/config.
git -c http.extraHeader="Authorization: Basic $AUTH" \
    push -q --force "$GITEA/$OBS_BOT_USER/${PKG_REPO#*/}.git" "HEAD:$BRANCH"

api() {
    curl -fsS -H "Authorization: token $OBS_BOT_TOKEN" -H "Content-Type: application/json" "$@"
}
TITLE="Update to $VERSION"
BODY=$(printf 'Automated snapshot of upstream main, %s.\n\nReview the .changes entry before merging:\n\n```\n%s\n```\n' \
    "https://github.com/lkocman/YAiST/commit/$COMMIT" "$CHANGES")
PAYLOAD=$(TITLE="$TITLE" BODY="$BODY" HEAD="$OBS_BOT_USER:$BRANCH" python3 -c '
import json, os
print(json.dumps({"title": os.environ["TITLE"], "body": os.environ["BODY"], "head": os.environ["HEAD"], "base": "main"}))')
OPEN=$(api "$GITEA/api/v1/repos/$PKG_REPO/pulls?state=open&limit=50" | BOT="$OBS_BOT_USER" BRANCH="$BRANCH" python3 -c '
import json, os, sys
for pr in json.load(sys.stdin):
    head = pr["head"]
    if head["ref"] == os.environ["BRANCH"] and (head.get("repo") or {}).get("owner", {}).get("login") == os.environ["BOT"]:
        print(pr["number"])
        break')
if [ -n "$OPEN" ]; then
    # The force-push already updated it; keep title and text current.
    api -X PATCH -d "$PAYLOAD" "$GITEA/api/v1/repos/$PKG_REPO/pulls/$OPEN" >/dev/null
    echo "Updated $GITEA/$PKG_REPO/pulls/$OPEN"
else
    NUM=$(api -X POST -d "$PAYLOAD" "$GITEA/api/v1/repos/$PKG_REPO/pulls" | python3 -c 'import json, sys; print(json.load(sys.stdin)["number"])')
    echo "Opened $GITEA/$PKG_REPO/pulls/$NUM"
fi
