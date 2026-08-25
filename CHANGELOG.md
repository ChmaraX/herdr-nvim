# Changelog

## [0.3.0](https://github.com/ChmaraX/herdr-nvim/compare/v0.2.1...v0.3.0) (2026-08-25)


### Features

* skip agent picker when target is unambiguous ([#17](https://github.com/ChmaraX/herdr-nvim/issues/17)) ([cf31879](https://github.com/ChmaraX/herdr-nvim/commit/cf3187992bbed14f628af045bba1179ba8efeae2))

## [0.2.1](https://github.com/ChmaraX/herdr-nvim/compare/v0.2.0...v0.2.1) (2026-08-20)


### Bug Fixes

* clamp pick-file scrape to herdr's cheap pane-read range ([#11](https://github.com/ChmaraX/herdr-nvim/issues/11)) ([994ff67](https://github.com/ChmaraX/herdr-nvim/commit/994ff67eba25856f77231b93fd1dd35377b5ef3d))

## [0.2.0](https://github.com/ChmaraX/herdr-nvim/compare/v0.1.1...v0.2.0) (2026-08-20)


### Features

* add open-link action and file-path/file-url link handlers ([ce83415](https://github.com/ChmaraX/herdr-nvim/commit/ce83415a504e3b7b8504282d893e33510d53fc02))
* build_candidates merge/dedup/order pipeline ([636256f](https://github.com/ChmaraX/herdr-nvim/commit/636256f03a22133a9fa86042ce15aaebdefb8828))
* claude session JSONL parser over verified fixture shapes ([71b4dcd](https://github.com/ChmaraX/herdr-nvim/commit/71b4dcddc548bd9f06cb066cf2be6e31d55db9ed))
* gather_candidates wires session mining + git status + scrape ([7b11700](https://github.com/ChmaraX/herdr-nvim/commit/7b11700f99efb7a4d2a84ec04640206b6cda2a21))
* hand-rolled ISO-8601 UTC timestamp parser ([ccf3100](https://github.com/ChmaraX/herdr-nvim/commit/ccf31000fce8df49718e0dfa3609efbe6369ed06))
* mine_session dispatcher with newly-created/last-edit reduction ([7c2ff12](https://github.com/ChmaraX/herdr-nvim/commit/7c2ff12ac03b0ec30ec6cbb77d859d08d2cf8827))
* net-change demotion rule for session-edited paths ([1ac9f15](https://github.com/ChmaraX/herdr-nvim/commit/1ac9f1598a17a49ccacb2d080293ba0a0239db61))
* parse clicked link text into (path, line) ([812d8a0](https://github.com/ChmaraX/herdr-nvim/commit/812d8a02681c66b7a3971201f9c0d508b9491ef3))
* parse git log --name-only for session-committed paths ([42de4bd](https://github.com/ChmaraX/herdr-nvim/commit/42de4bd34df90e83841ca3a40cfe4ee43a5a2756))
* parse git status --porcelain for dirty worktree paths ([c39ae89](https://github.com/ChmaraX/herdr-nvim/commit/c39ae89d5ec30a5c1ddd1f0417695b6eaebd1ff6))
* pi session JSONL parser over verified fixture shapes ([c2a83c3](https://github.com/ChmaraX/herdr-nvim/commit/c2a83c3f4222e543da52ce57c7659dcd1f039987))
* **picker:** fuzzy search, repo-wide fallback, and a cleaner cmd+o UI ([5f74f4c](https://github.com/ChmaraX/herdr-nvim/commit/5f74f4c0bf941a3e01506290f63f228f170db4cc))
* replace age with git diff stats, add picker title ([d80755c](https://github.com/ChmaraX/herdr-nvim/commit/d80755ca75718689266b264112b7a910287ced8a))
* resolve clicked paths against pane cwd then git toplevel ([2ed7643](https://github.com/ChmaraX/herdr-nvim/commit/2ed764325f8e8996a1440590ef7865dc71fa90f9))
* sectioned/scrollable picker UI with smart paths, badge, age ([b8a1e7b](https://github.com/ChmaraX/herdr-nvim/commit/b8a1e7b57d7b711243332527ecc9025da9a1bb2b))
* **sidebar:** add panel positioning ([#2](https://github.com/ChmaraX/herdr-nvim/issues/2)) ([40aadea](https://github.com/ChmaraX/herdr-nvim/commit/40aadeab3cef3702ef5e05069181c7168084794f))
* smart-path shortening, relative age, scroll-window math ([94541f2](https://github.com/ChmaraX/herdr-nvim/commit/94541f23094bf38ba6aea095fbdcf31fa7ced71c))
* surface pane agent_session via the Herdr trait ([35e19f9](https://github.com/ChmaraX/herdr-nvim/commit/35e19f97f3069535cd5301786df6221a2aaca42d))
* **ui:** redesign block annotations as sign-column rail + tint ([7956ea5](https://github.com/ChmaraX/herdr-nvim/commit/7956ea5ddc08f120466670d7504f87243c937c5e))
* whole-path case-insensitive filter with highlight spans ([a1cdc4a](https://github.com/ChmaraX/herdr-nvim/commit/a1cdc4abbd77fc3af443b1d7a46d4aa657de38e3))
* wire open-link subcommand into main.rs ([3b9daad](https://github.com/ChmaraX/herdr-nvim/commit/3b9daadbe9815c0d1d741b964a6e1fca56595185))


### Bug Fixes

* exec run.sh with bash, not sh (fixes [#3](https://github.com/ChmaraX/herdr-nvim/issues/3)) ([#4](https://github.com/ChmaraX/herdr-nvim/issues/4)) ([d23731f](https://github.com/ChmaraX/herdr-nvim/commit/d23731fa5c95124d53b006d6ba61b2fa7a404cd9))
* spawn the nvim daemon in the workspace's actual cwd, not inherited ([262bd3b](https://github.com/ChmaraX/herdr-nvim/commit/262bd3bdcc1a89b503261137ec86765f599930cb))
* use 'pane send-text' for paste, 'agent prompt' for send ([#1](https://github.com/ChmaraX/herdr-nvim/issues/1)) ([f9fb6e8](https://github.com/ChmaraX/herdr-nvim/commit/f9fb6e8e77bafbdd932a678ae20f99290c7e8d27))


### Performance Improvements

* merge pane_get calls and batch git diff stats; colorize diff counts ([ed74771](https://github.com/ChmaraX/herdr-nvim/commit/ed747714cbc2c6bdcfda9cfb1bcbe1e2a7e81c35))
