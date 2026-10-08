-- restask lock (§16): the lines of the TODO.md view that are not the user's to edit — the
-- frontmatter and the headings — are skipped by the cursor, as if they were not there, and
-- a view opens with the cursor on the line under `# TODO`, ready for a new task. The
-- functions down to `M.home` are pure — strings in, values out, no `vim` — and run under
-- the plain interpreter (`lua neovim/test/lock_test.lua`); the rest ties them to windows.
local start = require("restask.start")
local M = {}

--- The rows (1-based) of `lines` that are locked: the frontmatter block and every ATX
-- heading outside a fenced block (§6.2). Returns a set, `row -> true`.
---@param lines string[]
---@return table<integer, boolean>
function M.locked(lines)
	local out = {}
	local body = start.body_start(lines)
	for row = 1, body - 1 do
		out[row] = true
	end
	local in_fence = nil
	for row = body, #lines do
		local mark = start.fence(lines[row])
		if in_fence then
			if mark == in_fence then
				in_fence = nil
			end
		elseif mark then
			in_fence = mark
		elseif start.heading(lines[row]) then
			out[row] = true
		end
	end
	return out
end

--- The row a cursor that arrived on `row` belongs on: `row` itself when it is free,
-- else the nearest free row in the direction of travel (down when the cursor came from
-- above or `prev` is unknown), else the nearest free row the other way (a cursor
-- pushed against the top or the bottom stays on the first or last free row). Nil when every row is locked.
---@param lines string[]
---@param row integer
---@param prev integer|nil row the cursor came from
---@return integer|nil
function M.free(lines, row, prev)
	local locked = M.locked(lines)
	if not locked[row] then
		return row
	end
	local step = (prev and row < prev) and -1 or 1
	local function seek(from, dir)
		local at = from + dir
		while at >= 1 and at <= #lines do
			if not locked[at] then
				return at
			end
			at = at + dir
		end
		return nil
	end
	local found = seek(row, step)
	if found then
		return found
	end
	return seek(row, -step)
end

--- The row a view opens on: the line under its first heading (`# TODO`), where a new task
-- is typed. Nil when the view has no heading or nothing below it.
---@param lines string[]
---@return integer|nil
function M.home(lines)
	local locked = M.locked(lines)
	local body = start.body_start(lines)
	for row = body, #lines do
		if locked[row] then
			return row < #lines and row + 1 or nil
		end
	end
	return nil
end

--- Buffers whose cursor was put at home already.
local homed = {}

--- True when `buf` shows the TODO.md view of its vault.
local function is_view(buf)
	local kind = start.kind_of(buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
	return kind and kind.view or false
end

--- Keeps the cursor of the current window off the locked lines of a view.
function M.settle()
	local buf = vim.api.nvim_get_current_buf()
	local kind = vim.api.nvim_get_mode().mode:sub(1, 1)
	if (kind ~= "n" and kind ~= "i") or not is_view(buf) then
		return
	end
	local row, col = unpack(vim.api.nvim_win_get_cursor(0))
	local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
	local target = M.free(lines, row, vim.w.restask_row)
	if target and target ~= row then
		local width = #lines[target]
		pcall(vim.api.nvim_win_set_cursor, 0, { target, math.min(col, width) })
		row = target
	end
	vim.w.restask_row = row
end

--- Puts the cursor of the current window under `# TODO`, once per buffer.
local function open()
	local buf = vim.api.nvim_get_current_buf()
	if homed[buf] or not is_view(buf) then
		return
	end
	homed[buf] = true
	local row = M.home(vim.api.nvim_buf_get_lines(buf, 0, -1, false))
	if row then
		pcall(vim.api.nvim_win_set_cursor, 0, { row, 0 })
		vim.w.restask_row = row
	end
end

--- Registers the autocommands behind the rules.
function M.register()
	local group = vim.api.nvim_create_augroup("restask_lock", { clear = true })
	vim.api.nvim_create_autocmd("BufWinEnter", {
		group = group,
		callback = function()
			-- After the autocommands that restore the last cursor position.
			vim.schedule(open)
		end,
	})
	vim.api.nvim_create_autocmd({ "CursorMoved", "CursorMovedI" }, {
		group = group,
		callback = M.settle,
	})
	vim.api.nvim_create_autocmd({ "BufUnload", "BufWipeout" }, {
		group = group,
		callback = function(event)
			homed[event.buf] = nil
		end,
	})
end

return M
