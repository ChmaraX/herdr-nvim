-- Registers the :Herdr user command (deferred require). setup() re-registers
-- idempotently, covering eager, lazy, and daemon-injected installs alike.
require("herdr-nvim.commands").register()
