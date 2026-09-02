## Store app

- App ID:
- New or updated version:
- Public/demo smoke route:

## Author check

- [ ] I hand-authored only app source plus its source-adjacent `cobalt-app.json`; generated page/sitemap changes came from the check command.
- [ ] If release inputs changed, I added a meaningful `release_notes` entry and used the version requested by the check.
- [ ] `node tools/app-contribute.mjs --manifest apps/<id>/cobalt-app.json --dry-run` passes.
- [ ] Requested capabilities are necessary and the app has no embedded credentials.
- [ ] Screenshots and marketing routes contain no owner data or network identifiers.

## Policy review

- [ ] App identity, purpose, licensing, capabilities, and setup requirements are acceptable.
- [ ] CI formatting, tests, strict clippy, ARM verification, version gate, and generated pages pass.

Signing, Beta publication, catalog updates, provenance, smoke evidence, and Stable promotion are repository automation responsibilities. Contributors must not upload or sign release assets.
