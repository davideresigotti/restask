-- restask start (§16): a line opened in the TODO section of a note restask syncs starts
-- with a checkbox, as in the Obsidian plugin (§15.7). The functions down to `M.config`
-- are pure — strings in, values out, no `vim` — and run under the plain interpreter
-- (`lua neovim/test/start_test.lua`); what follows them ties the rule to the buffer.
local M = {}

--- What a new line in a TODO section starts with.
M.TASK_START = "- [ ] "

--- Lines of a note's head that are looked at for its frontmatter (§5.1).
local HEAD_LINES = 512

local function trim(text)
	return (text:gsub("^%s+", ""):gsub("%s+$", ""))
end

--- 1-based number of the first line after the frontmatter block (§5.1), 1 when there is
-- none.
---@param lines string[]
---@return integer
function M.body_start(lines)
	if not lines[1] or trim(lines[1]) ~= "---" then
		return 1
	end
	for i = 2, #lines do
		if trim(lines[i]) == "---" then
			return i + 1
		end
	end
	return 1
end

--- True when the note's own frontmatter routes it (§5.1): a non-empty `restask-list` or
-- `restask-list-root`.
---@param lines string[]
---@return boolean
function M.routes_itself(lines)
	for i = 2, M.body_start(lines) - 2 do
		local key, value = lines[i]:match("^([^:]*):(.*)$")
		key = key and trim(key)
		if (key == "restask-list" or key == "restask-list-root") and trim(value) ~= "" then
			return true
		end
	end
	return false
end

--- True when a note's head declares `restask-list-root` (§5.2).
---@param lines string[]
---@return boolean
function M.declares_root(lines)
	for i = 2, M.body_start(lines) - 2 do
		local key, value = lines[i]:match("^([^:]*):(.*)$")
		if key and trim(key) == "restask-list-root" and trim(value) ~= "" then
			return true
		end
	end
	return false
end

--- Rank and text of an ATX heading (§6.2), or nil when `line` is none.
local function heading(line)
	local marks, text = line:match("^(#+)[ \t]+(.-)[ \t]*#*[ \t]*$")
	if not marks or #marks > 6 or text == "" then
		return nil
	end
	return #marks, text
end

--- The character a fence line opens or closes a fenced block with (§6.2), or nil.
local function fence(line)
	local marks = line:match("^%s*(```+)") or line:match("^%s*(~~~+)")
	return marks and marks:sub(1, 1)
end

--- True when line `row` (1-based) lies in the TODO section of its note (§15.7). In a
-- note that is the lines below a heading whose text is `TODO` (in any letter case) down
-- to the next heading of the same or a higher level; in the TODO.md view (`view`) it is
-- the whole body. The done heading — `done` in a note, `Done` in the view — ends the
-- section in both, and the frontmatter and fenced blocks are never part of it.
---@param lines string[]
---@param row integer
---@param view boolean
---@param done string
---@return boolean
function M.in_todo(lines, row, view, done)
	local start = M.body_start(lines)
	if row < start then
		return false
	end
	if view then
		done = "Done"
	end
	local level = view and 0 or nil
	local in_fence = nil
	for i = start, math.min(row - 1, #lines) do
		local mark = fence(lines[i])
		if in_fence then
			if mark == in_fence then
				in_fence = nil
			end
		elseif mark then
			in_fence = mark
		else
			local rank, text = heading(lines[i])
			if rank then
				if text == done then
					return false
				end
				if not view then
					local todo = text:lower() == "todo"
					if not level then
						level = todo and rank or nil
					elseif rank <= level then
						level = todo and rank or nil
					end
				end
			end
		end
	end
	return level ~= nil and in_fence == nil
end

--- `line` with a checkbox behind its indentation when it is blank; nil when it has text.
---@param line string
---@return string|nil
function M.started(line)
	if line:find("[^ \t]") then
		return nil
	end
	return line .. M.TASK_START
end

--- What the rule needs from a vault's `restask.toml` (§14.1): `inbox_file` and
-- `done_heading`, with the engine's defaults. `text` nil — the vault has no config —
-- yields no inbox file: no file is the view then, as in the plugin (§15.6).
---@param text string|nil
---@return table
function M.config(text)
	if not text then
		return { done_heading = "Done" }
	end
	local function value(key)
		for line in (text .. "\n"):gmatch("(.-)\n") do
			local found = line:match("^[ \t]*" .. key .. '[ \t]*=[ \t]*"([^"]*)"')
			if found then
				return found
			end
		end
		return nil
	end
	return { inbox_file = value("inbox_file") or "TODO.md", done_heading = value("done_heading") or "Done" }
end

--- Line count of each buffer at the last look: a line was opened when it grew by one.
local counts = {}

--- How each buffer's note takes part, once asked: `{ view, done }`, or false when it does
-- not. Forgotten when the buffer is written or read again.
local kinds = {}

--- The first lines of the file at `path`, at most `HEAD_LINES`; nil when it cannot be read.
local function head(path)
	local file = io.open(path, "r")
	if not file then
		return nil
	end
	local lines = {}
	for line in file:lines() do
		lines[#lines + 1] = (line:gsub("\r$", ""))
		if #lines >= HEAD_LINES then
			break
		end
	end
	file:close()
	return lines
end

--- `path` with symlinks resolved, so that two spellings of one file compare equal.
local function real(path)
	return vim.uv.fs_realpath(path) or vim.fs.normalize(path)
end

--- True when a note in the folder of `file`, or in one above it up to `root`, declares
-- `restask-list-root` (§5.2).
local function inherits(file, root)
	local dir = vim.fs.dirname(file)
	while true do
		for name, kind in vim.fs.dir(dir) do
			if kind == "file" and name:sub(-3) == ".md" then
				local lines = head(dir .. "/" .. name)
				if lines and M.declares_root(lines) then
					return true
				end
			end
		end
		local parent = vim.fs.dirname(dir)
		if dir == root or parent == dir then
			return false
		end
		dir = parent
	end
end

--- How the note in `buf` takes part (invariant 4): `{ view, done }`, or false when it
-- is local-only or no Markdown file of a vault.
local function kind_of(buf, lines)
	if kinds[buf] ~= nil then
		return kinds[buf]
	end
	local kind = false
	local name = vim.api.nvim_buf_get_name(buf)
	if vim.bo[buf].filetype == "markdown" and name ~= "" then
		local file = real(name)
		local found = vim.fs.find({ "restask.toml", ".restask" }, { upward = true, path = vim.fs.dirname(file), limit = 1 })[1]
		if found then
			local root = vim.fs.dirname(found)
			local toml = io.open(root .. "/restask.toml", "r")
			local cfg = M.config(toml and toml:read("a") or nil)
			if toml then
				toml:close()
			end
			local view = cfg.inbox_file ~= nil and real(root .. "/" .. cfg.inbox_file) == file
			if view or M.routes_itself(lines) or inherits(file, root) then
				kind = { view = view, done = cfg.done_heading }
			end
		end
	end
	kinds[buf] = kind
	return kind
end

--- Looks at `buf` after a change made in insert mode, or on entering it: when the buffer
-- grew by exactly one line since the last look and the cursor is on a blank line of the
-- TODO section — `o`, `O`, Enter at the end of a line — that line gets its checkbox,
-- behind the indentation Neovim gave it, in the undo step of the line itself.
---@param buf integer
function M.open(buf)
	if not vim.api.nvim_buf_is_valid(buf) then
		return
	end
	local count = vim.api.nvim_buf_line_count(buf)
	local before = counts[buf]
	counts[buf] = count
	if not before or count ~= before + 1 or buf ~= vim.api.nvim_get_current_buf() then
		return
	end
	if vim.api.nvim_get_mode().mode:sub(1, 1) ~= "i" or not vim.bo[buf].modifiable then
		return
	end
	local row = vim.api.nvim_win_get_cursor(0)[1]
	local started = M.started(vim.api.nvim_get_current_line())
	if not started then
		return
	end
	local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
	local kind = kind_of(buf, lines)
	if not kind or not M.in_todo(lines, row, kind.view, kind.done) then
		return
	end
	pcall(vim.cmd, "undojoin")
	vim.api.nvim_buf_set_lines(buf, row - 1, row, false, { started })
	vim.api.nvim_win_set_cursor(0, { row, #started })
end

--- Registers the autocommands behind the rule.
function M.register()
	local group = vim.api.nvim_create_augroup("restask_start", { clear = true })
	local function look(event)
		counts[event.buf] = vim.api.nvim_buf_line_count(event.buf)
	end
	-- What is in the buffer when it is shown, read again or changed in normal mode is
	-- what a line opened afterwards is counted from.
	vim.api.nvim_create_autocmd({ "BufWinEnter", "BufReadPost", "FileChangedShellPost", "TextChanged", "InsertLeave" }, {
		group = group,
		callback = look,
	})
	vim.api.nvim_create_autocmd("TextChangedI", {
		group = group,
		callback = function(event)
			M.open(event.buf)
		end,
	})
	vim.api.nvim_create_autocmd("InsertEnter", {
		group = group,
		callback = function(event)
			-- `o` and `O` open their line before insert mode starts, and Neovim places
			-- the cursor only after this event.
			vim.schedule(function()
				M.open(event.buf)
			end)
		end,
	})
	-- The frontmatter, or a root note next to the file, may have changed.
	vim.api.nvim_create_autocmd({ "BufWritePost", "BufReadPost", "FileChangedShellPost" }, {
		group = group,
		callback = function(event)
			kinds[event.buf] = nil
		end,
	})
	vim.api.nvim_create_autocmd({ "BufUnload", "BufWipeout" }, {
		group = group,
		callback = function(event)
			counts[event.buf] = nil
			kinds[event.buf] = nil
		end,
	})
	for _, buf in ipairs(vim.api.nvim_list_bufs()) do
		if vim.api.nvim_buf_is_loaded(buf) then
			counts[buf] = vim.api.nvim_buf_line_count(buf)
		end
	end
end

return M
