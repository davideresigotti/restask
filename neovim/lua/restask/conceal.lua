-- restask conceal (§16): keeps the `🆔 restask-…` token of task lines off the screen and
-- out of reach. The token stays in the file — it links the line to its task on the
-- server. A window match conceals it in every mode, the cursor is kept off it, and an
-- edit that would damage it is repaired at once (the rules are in `guard.lua`).
local M = {}

local guard = require("restask.guard")

--- The token of §6.1 on a task line, with the one blank before it, as a Vim pattern.
M.PATTERN = [[^[ \t]*[-*+][ \t]\+\[[ xX]\][ \t]\+.\{-}\zs[ \t]\=🆔[ \t]\+\%(restask\|taskres\)-[0-9a-hjkmnp-tv-z]\{26}]]

--- The seal of a rendered view (§7.2), a frontmatter line the engine owns: hidden whole.
-- The line is not drawn at all (`conceal_lines`, Neovim 0.11), so no blank row is left.
M.SEAL = "^restask%-render: %x%x%x%x%x%x%x%x%x%x%x%x%x%x%x%x$"

--- The namespace of the seal's extmark (created on first use).
local function seal_ns()
	return vim.api.nvim_create_namespace("restask_seal")
end

--- Hides the seal line of the frontmatter of `buf`, or clears the hiding when there is none.
---@param buf integer
function M.hide_seal(buf)
	vim.api.nvim_buf_clear_namespace(buf, seal_ns(), 0, -1)
	local lines = vim.api.nvim_buf_get_lines(buf, 0, 64, false)
	if lines[1] ~= "---" then
		return
	end
	for i = 2, #lines do
		if lines[i] == "---" then
			return
		end
		if lines[i]:match(M.SEAL) then
			pcall(vim.api.nvim_buf_set_extmark, buf, seal_ns(), i - 1, 0, { end_row = i - 1, conceal_lines = "" })
			return
		end
	end
end

--- Modes in which the cursor line is concealed too; they are added to the window's
-- 'concealcursor'. All of them: the token is never shown, the guard protects it instead.
M.concealcursor = "nvic"

--- Whether edits that would damage a token are repaired and the cursor is kept off it.
M.guard = true

--- Buffer contents as of the last look, per guarded buffer: what an edit is compared to.
local shadows = {}

--- `current` with the modes of `wanted` it lacks appended.
---@param current string
---@param wanted string
---@return string
function M.with_modes(current, wanted)
	local out = current
	for mode in wanted:gmatch(".") do
		if not out:find(mode, 1, true) then
			out = out .. mode
		end
	end
	return out
end

--- True when `file` lies in a restask vault (a `restask.toml` or `.restask/` above it).
---@param file string absolute path
---@return boolean
function M.in_vault(file)
	if file == "" then
		return false
	end
	local found = vim.fs.find({ "restask.toml", ".restask" }, { upward = true, path = vim.fs.dirname(file), limit = 1 })
	return #found > 0
end

--- True when `buf` is a Markdown file of a vault.
local function is_note(buf)
	return vim.bo[buf].filetype == "markdown" and M.in_vault(vim.api.nvim_buf_get_name(buf))
end

--- Remembers what `buf` holds now; later edits are judged against it.
local function remember(buf)
	shadows[buf] = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
end

--- Conceals the tokens in the current window when it shows a Markdown file of a vault,
-- and undoes that when it shows anything else. 'conceallevel' is raised to 2 and
-- 'concealcursor' gains the modes above; both get their previous value back, unless
-- something else changed them in the meantime. In a vault window the two options are
-- brought back up whenever another plugin resets them (render-markdown.nvim does so on
-- every render), which is why this also runs on `OptionSet`.
function M.refresh()
	local buf = vim.api.nvim_get_current_buf()
	local wanted = is_note(buf)
	local state = vim.w.restask_conceal
	if wanted then
		M.hide_seal(buf)
		state = state
			or {
				match = vim.fn.matchadd("Conceal", M.PATTERN, 10, -1, { conceal = "" }),
				level = vim.wo.conceallevel,
				cursor = vim.wo.concealcursor,
			}
		state.set_level = math.max(vim.wo.conceallevel, 2)
		state.set_cursor = M.with_modes(vim.wo.concealcursor, M.concealcursor)
		if vim.wo.conceallevel ~= state.set_level then
			vim.wo.conceallevel = state.set_level
		end
		if vim.wo.concealcursor ~= state.set_cursor then
			vim.wo.concealcursor = state.set_cursor
		end
		vim.w.restask_conceal = state
		if M.guard and not shadows[buf] then
			remember(buf)
		end
	elseif state then
		pcall(vim.fn.matchdelete, state.match)
		vim.api.nvim_buf_clear_namespace(buf, seal_ns(), 0, -1)
		if vim.wo.conceallevel == state.set_level then
			vim.wo.conceallevel = state.level
		end
		if vim.wo.concealcursor == state.set_cursor then
			vim.wo.concealcursor = state.cursor
		end
		vim.w.restask_conceal = nil
	end
end

--- Keeps the cursor of the current window off the hidden token: in insert mode it never
-- rests inside or behind it, in normal mode never on it. Visual mode is left alone — a
-- selection may span the token, and what is then done to it goes through `M.protect`.
function M.settle()
	local buf = vim.api.nvim_get_current_buf()
	if not shadows[buf] then
		return
	end
	local mode = vim.api.nvim_get_mode().mode
	local kind = mode:sub(1, 1)
	if kind ~= "n" and kind ~= "i" and kind ~= "R" then
		return
	end
	local row, col = unpack(vim.api.nvim_win_get_cursor(0))
	local last = vim.w.restask_cursor
	local prev = last and last[1] == row and last[2] or nil
	local target = guard.settle(vim.api.nvim_get_current_line(), col, prev, kind ~= "n")
	local winline = vim.fn.winline()
	if target then
		-- `gj` onto a wrapped row that holds only the hidden token: the cursor went down a
		-- screen row from the line's last visible character, so it belongs on the next line.
		local down = kind == "n" and prev == target and col > target and vim.w.restask_winline and winline > vim.w.restask_winline
		if down and row < vim.api.nvim_buf_line_count(buf) then
			vim.cmd("normal! j")
			row, col = unpack(vim.api.nvim_win_get_cursor(0))
		else
			col = target
			vim.api.nvim_win_set_cursor(0, { row, col })
		end
		winline = vim.fn.winline()
	end
	vim.w.restask_cursor = { row, col }
	vim.w.restask_winline = winline
end

--- The line ranges in which `old` and `new` differ: a list of
-- `{ first old line, old count, first new line, new count }`, 1-based.
local function hunks(old, new)
	local diff = (vim.text and vim.text.diff) or vim.diff
	return diff(table.concat(old, "\n") .. "\n", table.concat(new, "\n") .. "\n", { result_type = "indices" })
end

--- Looks at what changed in `buf` since the last look and repairs every change that
-- damaged a token (`guard.repair`). Undo and redo are not judged: they restore text that
-- was valid before. The repair joins the undo step of the edit it corrects.
---@param buf integer
function M.protect(buf)
	local old = shadows[buf]
	if not old then
		return
	end
	local new = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
	local tree = vim.fn.undotree(buf)
	if tree.seq_cur ~= tree.seq_last or not vim.bo[buf].modifiable then
		shadows[buf] = new
		return
	end
	local current = buf == vim.api.nvim_get_current_buf()
	--- Repairs new lines `first`..`first + count - 1`, which were `before` (text).
	local function mend(before, first, count, keep)
		local fixed, caret = guard.repair(before, table.concat(new, "\n", first, first + count - 1), keep)
		if not fixed then
			return
		end
		pcall(vim.cmd, "undojoin")
		vim.api.nvim_buf_set_lines(buf, first - 1, first - 1 + count, false, vim.split(fixed, "\n", { plain = true }))
		if current and caret then
			local head = fixed:sub(1, caret)
			local _, breaks = head:gsub("\n", "")
			pcall(vim.api.nvim_win_set_cursor, 0, { first + breaks, #(head:match("[^\n]*$")) })
		end
	end
	local changed = hunks(old, new)
	-- Bottom-up, so the line numbers of the hunks above stay valid.
	for i = #changed, 1, -1 do
		local old_start, old_count, new_start, new_count = unpack(changed[i])
		if old_count == new_count then
			-- As many lines as before: each line is still there, so each keeps its token.
			for k = old_count - 1, 0, -1 do
				mend(old[old_start + k], new_start + k, 1, true)
			end
		elseif old_count > 0 and new_count > 0 then
			-- Lines were split or joined. (Lines that were only added or only removed
			-- cannot have damaged a token: a line takes its token with it.)
			mend(table.concat(old, "\n", old_start, old_start + old_count - 1), new_start, new_count, false)
		end
	end
	remember(buf)
	if current then
		M.settle()
	end
end

--- Registers the autocommands that keep every window up to date.
---@param opts table|nil `concealcursor` (modes to add to 'concealcursor'; default
--- "nvic", "" adds none) and `guard` (default true; false leaves edits and cursor alone)
function M.register(opts)
	opts = opts or {}
	if opts.concealcursor ~= nil then
		M.concealcursor = opts.concealcursor
	end
	if opts.guard ~= nil then
		M.guard = opts.guard
	end
	local group = vim.api.nvim_create_augroup("restask_conceal", { clear = true })
	vim.api.nvim_create_autocmd({ "BufWinEnter", "WinEnter", "FileType" }, {
		group = group,
		callback = M.refresh,
	})
	vim.api.nvim_create_autocmd("OptionSet", {
		group = group,
		pattern = { "conceallevel", "concealcursor" },
		callback = M.refresh,
	})
	vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI", "BufReadPost", "FileChangedShellPost" }, {
		group = group,
		callback = function(event)
			if vim.w.restask_conceal and event.buf == vim.api.nvim_get_current_buf() then
				M.hide_seal(event.buf)
			end
		end,
	})
	if M.guard then
		vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI" }, {
			group = group,
			callback = function(event)
				M.protect(event.buf)
			end,
		})
		vim.api.nvim_create_autocmd({ "CursorMoved", "CursorMovedI", "InsertEnter" }, {
			group = group,
			callback = function()
				-- `A` moves the cursor only after InsertEnter.
				vim.schedule(M.settle)
			end,
		})
		-- What comes from disk (a reload after the daemon or the CLI rewrote the note)
		-- is the new truth, not an edit.
		vim.api.nvim_create_autocmd({ "BufReadPost", "FileChangedShellPost" }, {
			group = group,
			callback = function(event)
				if shadows[event.buf] then
					remember(event.buf)
				end
			end,
		})
		vim.api.nvim_create_autocmd({ "BufUnload", "BufWipeout" }, {
			group = group,
			callback = function(event)
				shadows[event.buf] = nil
			end,
		})
	end
	M.refresh()
end

return M
