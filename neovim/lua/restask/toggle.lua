-- Restask toggle (§16): cursor-line toggle and inbox add, backed by the restask CLI.
-- Reads the cursor line/file and shells out to `restask done|undone|add`; the CLI
-- resolves the vault and does all parsing. The buffer is refreshed after a mutation.
local M = {}

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
	vim.cmd("checktime")
end

--- Toggles the task under the cursor (§16): an unchecked `[ ]` line runs
-- `restask done --file <bufname> --line <lnum>`, a checked `[x]`/`[X]` line runs
-- `restask undone …`; other lines are ignored. Refreshes the buffer on success.
function M.toggle()
	local line = vim.fn.getline(".")
	local lnum = vim.fn.line(".")
	local file = vim.fn.fnamemodify(vim.api.nvim_buf_get_name(0), ":p")
	local args
	if line:find("%[ %]", 1, true) then
		args = { BIN, "done", "--file", file, "--line", tostring(lnum) }
	elseif line:find("%[[xX]%]") then
		args = { BIN, "undone", "--file", file, "--line", tostring(lnum) }
	else
		return
	end
	run(args, function()
		refresh_buffer()
	end)
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

--- Registers the global keymaps (§16): `<leader>td` toggle, `<leader>ta` add.
function M.register_keymaps()
	vim.keymap.set("n", "<leader>td", M.toggle, { silent = true, desc = "Restask: toggle task" })
	vim.keymap.set("n", "<leader>ta", M.add, { silent = true, desc = "Restask: add task" })
end

return M
