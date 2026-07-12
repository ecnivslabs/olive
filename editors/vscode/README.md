# Olive for VSCode

Client for `pit lsp`: diagnostics as you type, hover, go-to-definition,
completion, and formatting.

## Try it locally

```
cd editors/vscode
npm install
code --extensionDevelopmentPath=. .
```

Requires `pit` on `PATH` (or set `olive.serverPath` in settings to its
location). Opens any `.liv` file to activate.

## Package

```
npx vsce package
```
