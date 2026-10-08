-- restask toggle (§16): cursor-line toggle and inbox add, backed by the restask CLI.
-- Reads the cursor line/file and shells out to `restask done|undone|add`; the CLI
-- resolves the vault and does all parsing. The buffer is refreshed after a mutation.
local M = {}

local settle = require("restask.settle")

local BIN = "restask"

--- Surfaces an error through `vim.notify` (§16 error surface).
---@param message string
local function notify_error(message)
	vim.notify(("restask: %s"):format(message), vim.log.levels.ERROR)
end

--- Runs `restask` asynchronously; `args` is the argv table (no shell, no quoting).
---@param args string[] argv, starting with the binary name.
---@param on_ok function|nil Called after a successful exit.
local function run(args, on_ok)
	if vim.fn.executable(BIN) ~= 1 then
		notify_error("`restask` binary not found in PATH")
		return
	end
	local stderr = {}
	local stdout = {}
	vim.fn.jobstart(args, {
		stdout_buffered = true,
		stderr_buffered = true,
		on_stdout = function(_, data)
			vim.list_extend(stdout, data)
		end,
		on_stderr = function(_, data)
			vim.list_extend(stderr, data)
		end,
		on_exit = function(_, code)
			local message = table.concat(stderr, "\n"):gsub("%s+$", "")
			if code == 0 then
				if on_ok then
					on_ok(table.concat(stdout, "\n"):gsub("%s+$", ""))
				end
			elseif message ~= "" then
				notify_error(message)
			else
				notify_error(("`%s` exited with code %d"):format(table.concat(args, " "), code))
			end
		end,
	})
end

--- Reloads the buffer from disk after the CLI mutated the file.
local function refresh_buffer()
	vim.cmd("silent! checktime")
end

--- Classifies a buffer line: "done" for an unchecked task, "undone" for a checked one,
-- nil for anything else. Mirrors the §6.1 task-line shape (`- [ ] `, `* [x] `, `+ [X] `).
---@param line string
---@return string|nil
function M.action_for(line)
	local box = line:match("^%s*[-*+]%s+%[([ xX])%]%s")
	if box == " " then
		return "done"
	elseif box then
		return "undone"
	end
	return nil
end

--- Toggles the task under the cursor (§16): an unchecked `[ ]` line runs
-- `restask done --file <absolute path> --line <lnum>`, a checked `[x]`/`[X]` line runs
-- `restask undone …`; other lines are ignored. The buffer is written first (the CLI
-- edits the file on disk) and reloaded on success. That write is not settled: the
-- command does the local work itself, and its `--line` is the line as written. The CLI
-- finds the vault from the file's own location, so Neovim's working directory does not
-- matter.
function M.toggle()
	local action = M.action_for(vim.fn.getline("."))
	if not action then
		return
	end
	local file = vim.fn.fnamemodify(vim.api.nvim_buf_get_name(0), ":p")
	if file == "" then
		notify_error("the buffer has no file")
		return
	end
	if vim.bo.modified then
		settle.without(function()
			vim.cmd("silent write")
		end)
	end
	run({ BIN, action, "--file", file, "--line", tostring(vim.fn.line(".")) }, refresh_buffer)
end

--- Prompts for a task and appends it to the TODO.md inbox: `restask add "<text>"`.
function M.add()
	local text = vim.fn.input("Task: ")
	if text == "" then
		return
	end
	run({ BIN, "add", text }, function(uid)
		vim.notify(("restask: added %s"):format(uid), vim.log.levels.INFO)
	end)
end

--- The prefix the keymaps share, and the name its group goes by in a key hint.
M.PREFIX = "<leader>t"
M.GROUP = "restask"

--- Names the keymaps' prefix in which-key (§16), so that the hint shown after
-- `<leader>` reads `t → restask` and not a count of keymaps. `wk` is the which-key
-- module: `add` is its current interface, `register` the one before version 3. True when
-- the group was named; a `wk` with neither, or one that fails, is left alone.
---@param wk table|nil
---@return boolean
function M.name_group(wk)
	if type(wk) ~= "table" then
		return false
	end
	if type(wk.add) == "function" then
		return (pcall(wk.add, { { M.PREFIX, group = M.GROUP } }))
	end
	if type(wk.register) == "function" then
		return (pcall(wk.register, { [M.PREFIX] = { name = M.GROUP } }))
	end
	return false
end

--- Registers the global keymaps (§16): `<leader>td` toggle, `<leader>ta` add. Where the
-- configuration has which-key, their prefix is named there; without it nothing is asked.
function M.register_keymaps()
	vim.keymap.set("n", M.PREFIX .. "d", M.toggle, { silent = true, desc = "restask: toggle task" })
	vim.keymap.set("n", M.PREFIX .. "a", M.add, { silent = true, desc = "restask: add task" })
	local found, wk = pcall(require, "which-key")
	if found then
		M.name_group(wk)
	end
end

return M
