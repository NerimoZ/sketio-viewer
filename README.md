<h1><img src="ui/logo.png" alt="" width="40" height="40" align="absmiddle"> Sketio Viewer</h1>

A small Windows editor and reader for local Markdown and plain text files. It uses the Windows RichEdit control and runs without WebView2.

## Install

Download the [Windows x64 installer](../../releases/latest) and run it for the current user. The release also includes a standalone EXE. Choose **Sketio Viewer** in **Settings → Apps → Default apps** to make it the default for `.md`, `.markdown`, and `.txt`. Explorer displays the transparent Sketio icon for those file types when Viewer is selected.

## Use

Launch the viewer or pass a Markdown or TXT file on the command line.

```powershell
& .\target\release\SketioViewer.exe .\README.md
```

| Shortcut | Action |
| --- | --- |
| Ctrl+N | New text tab |
| Ctrl+Shift+N | New window |
| Ctrl+O | Open Markdown or TXT |
| Ctrl+S | Save changes |
| Ctrl+Shift+S | Save as |
| Ctrl+Alt+S | Save all modified tabs |
| Ctrl+P | Print |
| Ctrl+R | Reload the current file |
| Ctrl+G | Go to line |
| F5 | Insert local time and date |
| Ctrl+F | Find in the current document |
| Ctrl+H | Find and replace |
| F3 / Shift+F3 | Next / previous match |
| Alt+Left | Return to the previous document |
| Ctrl+Shift+L | Switch light and dark |

The viewer restores your tabs at launch and reads background documents when you first select them.

Drop a Markdown or TXT file into the window to open it. TXT files always display as plain text. In Markdown files, the line with the caret shows the original markup while headings, wrapping tables, nested lists, task checkboxes, quotes, and inline styles stay formatted on surrounding lines. Use the **Markdown on/off** button in the status bar or **View → Markdown formatting** to switch between live formatting and plain source.

Use **View → Reading view** for embedded images and clickable links. Relative assets and documents work offline. Web links open in your default browser.

## Build

Build the native viewer on Windows x64 with the Rust MSVC toolchain.

```powershell
cargo test --locked --bin SketioViewer
cargo build --release --locked --bin SketioViewer
```

Build the Windows installer with Inno Setup 6 installed.

```powershell
pwsh -File .\installer\build.ps1
```
