# Reading rules

Lightair fetches `rules.json` from here (every six hours) and uses it when its signature,
`rules.json.sig`, checks out against the public key built into Lightair
(`crates/lightair-live/src/rules.rs`). It fixes a chat reader or sender when a site changes,
without a new release.

- `version`: a whole number, higher than the last published one, or Lightair ignores the file.
- `off`: platforms whose reader and sender Lightair turns off while they're broken
  (`["tiktok"]`).
- `values`: rules by name, overriding the built-in ones (`"tiktok.chat_container": "…"`,
  `"youtube.send_input": "…"`; the names are the `rules::text` calls in the code).

To publish a change: edit `rules.json` (raise `version`), then

    cargo xtask rules-sign rules/rules.json

(it signs with the private key in the publisher's Keychain) and push both files to `main`.
