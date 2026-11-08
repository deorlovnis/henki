# prompts: /review, /explain, /commit-msg and /fix-test send a prepared prompt; text after the command is added to it

Commands for you, not for the model (they cost no prompt bytes). Each shows its prompt and asks `send? [Y/n]`; Enter
sends it as your next message. Text after the command goes at the end, after a blank line:

- `/review [FOCUS]`: review the uncommitted changes for bugs, risks and missing tests.
- `/explain [WHAT]`: explain how a part of this project works, with the files and lines it rests on.
- `/commit-msg [HINT]`: write a commit message for the staged changes.
- `/fix-test [TEST]`: run the failing test, find the cause and fix it.

Each prompt is the file `cmd/NAME.md`. Your own, in a module of your own named MINE:

1. `cp -R ~/.config/henki/ext/prompts MINE` and `chmod -R u+w MINE` (the store copy is read-only).
2. Set `name = "MINE"` in `MINE/.henki/module.toml`.
3. Remove the four official commands, `cmd/review`, `cmd/explain`, `cmd/commit-msg`, `cmd/fix-test` and their `.md`
   files: a command name in two modules stops henki at start (or keep only your copy on).
4. Add `cmd/NAME.md` and a link: `ln -s ../.henki/template MINE/cmd/NAME`.
5. `henki mods add ./MINE`.
