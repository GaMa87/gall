# German and Spanish interface glossary

This glossary keeps the German and Spanish UI consistent. It covers Gall's interface and Git concepts; names of files, folders, drives, branches, remotes and programs remain user data and are never translated.

| English concept | German | Spanish |
|---|---|---|
| file | Datei | archivo |
| folder | Ordner | carpeta |
| drive / volume | Laufwerk / Datenträger | unidad / volumen |
| tab | Tab | pestaña |
| browser pane | Browserbereich | panel del explorador |
| window | Fenster | ventana |
| Trash | Papierkorb | Papelera |
| Git branch | Branch | rama |
| Git stash | Stash | stash |
| rebase | Rebase | rebase |
| commit | Commit | commit |
| working tree | Arbeitsverzeichnis | árbol de trabajo |
| undo | Rückgängig | deshacer |
| compare | vergleichen / Vergleich | comparar / comparación |

## Review notes

- Keep keyboard key names and shortcut notation recognizable; translate the action around them, not the key itself.
- Preserve placeholders such as `{name}`, `{count}` and `{path}` exactly. File paths, command output, code, command names and user supplied labels stay verbatim.
- Use concise labels in menus and settings; use complete sentences for warnings and explanations. Keep the same informal, direct tone across both languages.
- German and Spanish catalogs cover the complete current UI key set and are checked for missing keys and placeholder mismatches. Before a public release, a native speaker should review idiom, grammatical gender and number, Git terminology, and long labels in the screenshots from `scripts/selftest.sh --i18n`.
