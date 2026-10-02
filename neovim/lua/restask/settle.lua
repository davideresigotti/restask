-- restask settle (§16): the local work of the vault, done when a note is written. The
-- rules are the engine's: `restask settle` registers, repairs and files the lines and
-- refreshes TODO.md, without a server and without a daemon on this machine. The buffers
-- are reloaded from what it left.
local M = {}

local conceal = require("restask.conceal")

local BIN = "restask"

--- How long a write waits for the CLI before giving up on it, in milliseconds.
M.TIMEOUT_MS = 5000

--- Writes made while this is set are not settled (see `M.without`).
local paused = false

--- Whether the missing binary has been reported; once is enough for a session.
local reported = false

--- The file's modification time when its buffer was last read or settled, per buffer:
-- a file that still has it has not been written since.
local stamps = {}

--- Events after which a written buffer is looked for (see `M.register`).
M.EVENTS = { "InsertLeave", "TextChanged", "TextChangedI", "BufLeave", "FocusLost" }

--- The modification time of `file` as a comparable string, nil when it cannot be read.
---@param file string
---@return string|nil
local function stamp(file)
	local stat = vim.uv.fs_stat(file)
	return stat and ("%d.%09d"):format(stat.mtime.sec, stat.mtime.nsec)
end

--- The command that settles the vault `file` is in: an argv table (no shell, no quoting).
---@param file string absolute path of a note
---@return string[]
function M.command(file)
	return { BIN, "settle", "--file", file }
end

--- Runs `fn` with settling switched off: for a write that is followed by a CLI command
-- of its own, which addresses a line of the file as it was written.
---@param fn function
function M.without(fn)
	paused = true
	local ok, err = pcall(fn)
	paused = false
	if not ok then
		error(err, 0)
	end
end

--- Settles the vault of `buf` if it is a Markdown file of one, and reloads the buffers
-- the CLI rewrote. The CLI is waited for — it takes a few milliseconds — so that the
-- buffer cannot be edited between the write and the reload.
---@param buf integer
function M.run(buf)
	if paused or vim.bo[buf].filetype ~= "markdown" then
		return
	end
	local file = vim.fn.fnamemodify(vim.api.nvim_buf_get_name(buf), ":p")
	if not conceal.in_vault(file) then
		return
	end
	if vim.fn.executable(BIN) ~= 1 then
		if not reported then
			reported = true
			vim.notify("restask: `restask` binary not found in PATH", vim.log.levels.ERROR)
		end
		return
	end
	local result = vim.system(M.command(file), { text = true }):wait(M.TIMEOUT_MS)
	stamps[buf] = stamp(file)
	if result.code == 0 then
		vim.cmd("silent! checktime")
		return
	end
	local message = (result.stderr or ""):gsub("%s+$", "")
	if message == "" then
		message = ("`restask settle` exited with code %d"):format(result.code)
	end
	vim.notify(("restask: %s"):format(message), vim.log.levels.ERROR)
end

--- Settles `buf` if its file was written since the last look: the buffer is saved and
-- the file is not the one that was read or settled.
---@param buf integer
function M.catch_up(buf)
	if not vim.api.nvim_buf_is_loaded(buf) or vim.bo[buf].modified or vim.bo[buf].filetype ~= "markdown" then
		return
	end
	local at = stamp(vim.fn.fnamemodify(vim.api.nvim_buf_get_name(buf), ":p"))
	if at and at ~= stamps[buf] then
		M.run(buf)
	end
end

--- Registers the autocommands: every written buffer is settled. `BufWritePost` is the
-- write itself. It does not fire for a write made from inside another autocommand that
-- is not `nested` — the usual auto-save recipe (`update` on `InsertLeave`) — so after
-- the events such recipes use, once their autocommands have run, a buffer whose file
-- changed under it is settled too.
function M.register()
	local group = vim.api.nvim_create_augroup("restask_settle", { clear = true })
	vim.api.nvim_create_autocmd("BufWritePost", {
		group = group,
		callback = function(event)
			M.run(event.buf)
		end,
	})
	vim.api.nvim_create_autocmd(M.EVENTS, {
		group = group,
		callback = function(event)
			vim.schedule(function()
				M.catch_up(event.buf)
			end)
		end,
	})
	-- What was read from disk is not a write of this editor.
	vim.api.nvim_create_autocmd({ "BufReadPost", "FileChangedShellPost" }, {
		group = group,
		callback = function(event)
			stamps[event.buf] = stamp(vim.fn.fnamemodify(vim.api.nvim_buf_get_name(event.buf), ":p"))
		end,
	})
	vim.api.nvim_create_autocmd({ "BufUnload", "BufWipeout" }, {
		group = group,
		callback = function(event)
			stamps[event.buf] = nil
		end,
	})
end

return M
