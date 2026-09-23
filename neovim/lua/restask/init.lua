-- Taskres Neovim integration (§16): thin wrapper over the restask CLI.
-- No Markdown parsing here — all logic lives in the CLI (zero drift).
local M = {}

local toggle = require("restask.toggle")

--- Sets up the integration: `require("restask").setup({ keymaps = true })`.
--
-- Options:
--   { keymaps = true }  register the global keymaps (§16):
--     `<leader>td`  toggle the task under the cursor (done/undone)
--     `<leader>ta`  prompt for a task and append it to the TODO.md inbox
--
-- Errors from the CLI are surfaced through `vim.notify` (§16).
---@param opts table|nil Options table; `keymaps` defaults to true.
function M.setup(opts)
	opts = opts or {}
	if opts.keymaps == nil then
		opts.keymaps = true
	end
	if opts.keymaps then
		toggle.register_keymaps()
	end
end

return M
