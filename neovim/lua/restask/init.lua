-- restask Neovim integration (§16): thin wrapper over the restask CLI.
-- No Markdown parsing here — all logic lives in the CLI (zero drift).
local M = {}

local toggle = require("restask.toggle")
local conceal = require("restask.conceal")

--- Sets up the integration: `require("restask").setup({ keymaps = true })`.
--
-- Options:
--   { keymaps = true }  register the global keymaps (§16):
--     `<leader>td`  toggle the task under the cursor (done/undone)
--     `<leader>ta`  prompt for a task and append it to the TODO.md inbox
--   { conceal = true }  hide the `🆔 restask-…` token of task lines in vault notes
--   { concealcursor = "nc" }  modes in which the cursor line hides it too (§16)
--
-- Errors from the CLI are surfaced through `vim.notify` (§16).
---@param opts table|nil Options table; `keymaps` and `conceal` default to true.
function M.setup(opts)
	opts = opts or {}
	if opts.keymaps == nil then
		opts.keymaps = true
	end
	if opts.keymaps then
		toggle.register_keymaps()
	end
	if opts.conceal ~= false then
		conceal.register(opts.concealcursor)
	end
end

return M
