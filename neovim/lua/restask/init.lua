-- restask Neovim integration (§16): thin wrapper over the restask CLI.
-- No Markdown parsing here — all logic lives in the CLI (zero drift).
local M = {}

local toggle = require("restask.toggle")
local conceal = require("restask.conceal")
local start = require("restask.start")
local suggest = require("restask.suggest")
local settle = require("restask.settle")

--- Sets up the integration: `require("restask").setup({ keymaps = true })`.
--
-- Options:
--   { keymaps = true }  register the global keymaps (§16):
--     `<leader>td`  toggle the task under the cursor (done/undone)
--     `<leader>ta`  prompt for a task and append it to the TODO.md inbox
--   { conceal = true }  hide the `🆔 restask-…` token of task lines in vault notes,
--     in every mode, and keep it from being edited (§16)
--   { guard = true }  false: only hide the token, do not protect it
--   { concealcursor = "nvic" }  modes in which the cursor line hides it too
--   { start_tasks = true }  a line opened in the TODO section of a note restask syncs
--     starts with `- [ ] ` (§16)
--   { suggest = true }  typing two or more letters of a metadata keyword in a task
--     line (`hi`, `du`, `tom`…) opens the blink.cmp menu with the matching tokens (§16)
--   { settle = true }  writing a vault note does restask's local work at once, through
--     the CLI: new lines are registered, a checked one moves under the done heading,
--     TODO.md follows — no daemon, no network (§16)
--
-- Errors from the CLI are surfaced through `vim.notify` (§16).
---@param opts table|nil Options table; `keymaps`, `conceal`, `start_tasks`, `suggest` and `settle` default to true.
function M.setup(opts)
	opts = opts or {}
	if opts.keymaps == nil then
		opts.keymaps = true
	end
	if opts.keymaps then
		toggle.register_keymaps()
	end
	if opts.conceal ~= false then
		conceal.register({ concealcursor = opts.concealcursor, guard = opts.guard })
	end
	if opts.start_tasks ~= false then
		start.register()
	end
	if opts.suggest ~= false then
		suggest.register()
	end
	if opts.settle ~= false then
		settle.register()
	end
end

return M
