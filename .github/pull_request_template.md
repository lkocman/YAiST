## What and why

<!-- What does this change, and why? Link issues (Fixes #123). -->

## Screenshots

<!-- Required when the desktop or terminal client changes: before and after.
     The "PR checks" job looks for an image here. Hard-to-reach states:
     YAIST_DEMO_SCENE (crates/yaist-ui/src/demo.rs) and scripts/screenshots.sh.
     Nothing visible changed? Add the no-screenshot label. -->

## How was it tested

<!-- Which distributions (container or real system), which commands or
     requests did you try? -->

## Checklist

- [ ] New or changed actions have the right risk level (anything that changes the system is not `Read`)
- [ ] Arguments that reach commands or D-Bus are validated in `yaist-core/src/validate.rs`
- [ ] Works on Leap and Tumbleweed (and transactional systems if packages are involved)
- [ ] New dependencies: `scripts/license-report.sh` output checked, `docs/LICENSING.md` updated if the licence set changed
- [ ] Handbook up to date (`yaist docs`), if actions changed
