ci:
	cargo fmt --check
	cargo test
	nvim --headless --noplugin -u NONE -l tests/run.lua
