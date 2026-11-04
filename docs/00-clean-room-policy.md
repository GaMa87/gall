# Clean-room policy

Gall is an independent, open-source file manager. Its workflows are
inspired by the publicly documented behaviour of OneCommander (© Tangible Loop
LLC). It contains no code, assets, or text from that product.

## Rules

1. **No decompilation or disassembly** of OneCommander binaries (`*.dll`, `*.exe`,
   `*.pdb`, satellite resource assemblies). The OneCommander EULA forbids it,
   and the result would not be clean-room.
2. **Allowed sources of information**
   - The public manual (`OneCommanderHelp.chm`, `onecommander.com`), the
     public release notes, and the product web pages.
   - Plain-text instructions shipped for end users (e.g. *How to customize
     icons.txt*). These describe user-facing file and folder conventions.
   - The third-party dependency names and versions in the shipped
     `*.deps.json`. This shows *which kinds* of capability exist, such as
     PDF preview or video preview.
   - Black-box observation of the running program (screenshots, notes, timing)
     by a licensed user. This is the "observe, study and test" right in
     EU Directive 2009/24/EC, Art. 5(3).
3. **No copying** of themes, icons, translations, templates, screenshots or
   manual text into this repository. UI components, icons and strings are
   written independently.
4. **Interoperability formats**, such as importing OneCommander favourites or
   `.2do` notes, may be implemented from observed file contents created by
   the user's own data. Each format must be documented in `docs/formats/`
   with its source.
5. Each doc separates **[Observed]** facts (with a source reference) from
   **[Hypothesis]** design guesses.

## Source reference keys

| Key | Source |
|-----|--------|
| `M:<page>` | Official manual page, e.g. `M:tabs` = *tabs.htm* in `OneCommanderHelp.chm` |
| `RN:<ver>` | Public release notes entry for that version |
| `WEB:<path>` | onecommander.com page |
| `PKG:<file>` | User-facing text/file layout in the v4 beta 0.0.78.0 ZIP (not binaries) |
| `OBS:<id>` | Black-box observation by the maintainer on Windows (see `observation-requests.md`) |
