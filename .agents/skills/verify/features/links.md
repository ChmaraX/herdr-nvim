# File links

Ctrl+click a file path in any pane to open it in the sidebar at that line.

## Sub-features

- path links: text such as `src/a.rs:12:3`. A path needs a directory segment and an extension, so a bare `README.md` does not match.
- osc8 links: `file://` hyperlinks that an agent prints.
- resolution: relative paths are tried against the pane's cwd, then against the git root. If a path does not resolve, nothing happens and no message is shown.
- open: the sidebar opens if needed and shows the file at that line.

## How to get to it (user POV)

Ctrl+click a path in any herdr pane. The terminal must pass the click through to herdr.

## Driving it in the box

VHS has no mouse, so a real Ctrl+click is not possible. The closest real path is herdr's click handler called with a faked click. It runs the same open path. Take the ids from `herdr pane list`:

```sh
box exec <item> -- sh -c 'cd ~/demo && HERDR_PLUGIN_CLICKED_URL=src/main.js:2 HERDR_PANE_ID=w1:p1 HERDR_WORKSPACE_ID=w1 HERDR_TAB_ID=w1:t1 /opt/herdr-nvim/bin/herdr-nvim open-link'
```

Then check the sidebar with `box exec <item> -- herdr pane read <sidebar-pane-id> --source visible`: the last line is nvim's statusline with the file and line. Use `file:///home/box/demo/src/greet.js` for an osc8 link and `src/nope.js:3` for a path that does not resolve. Which text gets linkified is herdr's job (the plugin's `link_handlers` regex); `cargo test` covers it (for code you did not write, run it with `box test <item>`, never on the host).

## Gotchas

- Web links (`https://…`) must still open the browser and must not be grabbed as file paths. The box has no browser, so this cannot be observed there.
- Windows `C:\…` paths must still match, and this cannot be checked in the box either.
- When a path does not resolve there is no error, so a sidebar that stays closed is the only signal.
- Opening into an already-open sidebar leaves focus on the clicked pane, and the handler prints "could not focus sidebar pane" (herdr 0.9.3 refuses to focus a non-agent pane). A sidebar the link opens is focused.
- Cmd+Shift+click in Ghostty does not work today. Do not test it.
