-- restask guard (§16): the rules that keep a hidden `🆔` token from being edited.
-- Pure Lua — strings in, strings out, no `vim` — so it runs under the plain interpreter.
-- The same rules as the Obsidian plugin's editor guard (§15.5): a token leaves its line
-- only together with the task's whole body, and a line break never carries it along.
-- Offsets are 0-based byte offsets; ranges are half-open.
local M = {}

local EMOJI = "🆔"

--- Offset at which the body of a task line starts (§6.1), or nil when it is not a task.
---@param line string
---@return integer|nil
function M.body_start(line)
	local pos = line:match("^[ \t]*[-*+][ \t]+%[[ xX]%][ \t]+()")
	return pos and pos - 1
end

--- The token that gives a task line its UID, with the one blank before it: `body` (start
-- of the line's body), `start`..`stop` (the hidden range), `emoji` (where `🆔` is). Further
-- blanks are text: hidden, a blank typed at the visible end would take the cursor with it.
-- Nil when the line is not a task or its first token is not a valid UID.
---@param line string
---@return table|nil
function M.token(line)
	local body = M.body_start(line)
	if not body then
		return nil
	end
	local at = line:find(EMOJI, body + 1, true)
	while at do
		local uid = line:match("^[ \t]+()", at + #EMOJI)
		if uid then
			local prefix = line:sub(uid, uid + 7)
			local id = line:sub(uid + 8, uid + 33)
			if (prefix == "restask-" or prefix == "taskres-") and #id == 26 and not id:find("[^0-9a-z]") then
				-- Crockford base32 has no i, l, o, u: such a token is not a UID (§3.1).
				if id:find("[ilou]") then
					return nil
				end
				local start = at - 1
				if start > body and (line:byte(start) == 32 or line:byte(start) == 9) then
					start = start - 1
				end
				return { body = body, start = start, emoji = at - 1, stop = uid + 33 }
			end
		end
		at = line:find(EMOJI, at + #EMOJI, true)
	end
	return nil
end

local function continuation(byte)
	return byte ~= nil and byte >= 0x80 and byte < 0xC0
end

--- The lines of `text` with their offsets and, in offsets of `text`, their tokens.
local function scan(text)
	local lines, from = {}, 0
	for line in (text .. "\n"):gmatch("(.-)\n") do
		local entry = { from = from, to = from + #line }
		local token = M.token(line)
		if token then
			entry.token = {
				body = from + token.body,
				start = from + token.start,
				emoji = from + token.emoji,
				stop = from + token.stop,
				line_to = entry.to,
				text = line:sub(token.emoji + 1, token.stop),
			}
		end
		lines[#lines + 1] = entry
		from = entry.to + 1
	end
	return lines
end

local function line_at(lines, pos)
	for _, line in ipairs(lines) do
		if pos <= line.to then
			return line
		end
	end
	return lines[#lines]
end

--- What turned `old` into `new`, read as one replacement of `from`..`to` by `insert`.
local function difference(old, new)
	local limit = math.min(#old, #new)
	local from = 0
	while from < limit and old:byte(from + 1) == new:byte(from + 1) do
		from = from + 1
	end
	while from > 0 and (continuation(old:byte(from + 1)) or continuation(new:byte(from + 1))) do
		from = from - 1
	end
	local tail = 0
	while tail < limit - from and old:byte(#old - tail) == new:byte(#new - tail) do
		tail = tail + 1
	end
	while tail > 0 and (continuation(old:byte(#old - tail + 1)) or continuation(new:byte(#new - tail + 1))) do
		tail = tail - 1
	end
	return from, #old - tail, new:sub(from + 1, #new - tail)
end

--- Repairs an edit that damaged a hidden token. `old` and `new` are the text of the
-- changed lines before and after it, `lines` the scan of `old`. Returns the text to have
-- instead and the offset in it at which the cursor belongs, or nil when no token was
-- damaged.
local function mend(lines, old, new, keep)
	local from, to, insert = difference(old, new)

	-- Text typed in front of the token that ends in a blank reads the same as text typed
	-- behind the token's first blank; take the reading that leaves the token whole.
	if from == to and not insert:find("\n", 1, true) then
		local token = line_at(lines, from).token
		if token and from <= token.emoji then
			while from > token.start and old:sub(from, from) == insert:sub(-1) do
				insert = insert:sub(-1) .. insert:sub(1, -2)
				from = from - 1
			end
			to = from
		end
	elseif from == to then
		-- A line break in front of the token: the blanks between them belong to what was
		-- typed (Vim drops the old ones when it splits a line and adds the list leader's).
		local token = line_at(lines, from).token
		if token and from >= token.start and from < token.emoji then
			insert = insert .. old:sub(from + 1, token.emoji)
			from, to = token.start, token.start
		end
	end

	local whole = { from = from, to = to, insert = insert }
	local edits = { whole }
	local caret = { edit = whole }
	local altered = false

	if from == to then
		local token = line_at(lines, from).token
		if token and not insert:find("\n", 1, true) and from > token.start and from <= token.stop then
			whole.from, whole.to = token.start, token.start
			altered = true
		end
	else
		local first, last = line_at(lines, from), line_at(lines, to)
		local damaged = {}
		for _, line in ipairs(first == last and { first } or { first, last }) do
			local token = line.token
			local whole_body = token and not keep and from <= token.body and to >= token.stop
			if token and from < token.stop and to > token.start and not whole_body then
				-- The change spans the token but writes it back: nothing was lost.
				if insert:find(token.text, 1, true) then
					return nil
				end
				damaged[#damaged + 1] = token
			end
		end
		if #damaged > 0 then
			altered = true
			edits = {}
			local inside = from >= damaged[1].start and to <= damaged[1].stop
			local pos = from
			for _, token in ipairs(damaged) do
				if token.start > pos then
					edits[#edits + 1] = { from = pos, to = token.start, insert = "" }
				end
				pos = math.max(pos, token.stop)
			end
			if to > pos then
				edits[#edits + 1] = { from = pos, to = to, insert = "" }
			end
			if inside and insert:find("\n", 1, true) then
				-- A line break that replaced one of the token's blanks (Vim drops the
				-- blank when it splits a line): a break at the token, handled below.
				edits = { { from = damaged[1].start, to = damaged[1].start, insert = insert } }
				caret = { edit = edits[1] }
			elseif inside then
				caret = { pos = damaged[1].start }
			elseif insert ~= "" then
				local at = math.min(from, damaged[1].start)
				if edits[1] and edits[1].from == at then
					edits[1].insert = insert
				else
					table.insert(edits, 1, { from = at, to = at, insert = insert })
				end
				caret = { edit = edits[1] }
			elseif edits[1] then
				caret = { edit = edits[1] }
			else
				caret = { pos = damaged[1].start }
			end
		end
	end

	local out = {}
	for _, edit in ipairs(edits) do
		-- Only a break put into the token's own line can carry the token away.
		local line = edit.insert:find("\n", 1, true) and line_at(lines, edit.to)
		local token = line and line.from <= edit.from and line.token
		local cut = token and edit.insert:find("\n", 1, true)
		if token and edit.to >= token.start and (edit.to < token.stop or (edit.to == token.stop and cut > 1)) then
			altered = true
			local head = {
				from = math.min(edit.from, token.start),
				to = math.min(edit.to, token.start),
				insert = edit.insert:sub(1, cut - 1),
			}
			local tail = { from = token.stop, to = token.stop, insert = edit.insert:sub(cut) }
			if head.from < head.to or head.insert ~= "" then
				out[#out + 1] = head
			end
			out[#out + 1] = tail
			if caret.edit == edit then
				caret = { edit = tail }
			end
		else
			out[#out + 1] = edit
		end
	end
	if not altered then
		return nil
	end

	local parts, pos, target = {}, 0, nil
	local length = 0
	for i, edit in ipairs(out) do
		local next_edit = out[i + 1]
		-- A moved line break may meet the rest of a cut deletion at the token's end.
		if next_edit and edit.from == edit.to and next_edit.from == edit.from and next_edit.insert == "" then
			next_edit.insert = edit.insert
			if caret.edit == edit then
				caret = { edit = next_edit }
			end
		else
			local kept = old:sub(pos + 1, edit.from)
			if caret.pos and caret.pos >= pos and caret.pos <= edit.from and not target then
				target = length + caret.pos - pos
			end
			parts[#parts + 1] = kept
			parts[#parts + 1] = edit.insert
			length = length + #kept + #edit.insert
			if caret.edit == edit then
				target = length
			end
			pos = edit.to
		end
	end
	if caret.pos and not target then
		target = length + caret.pos - pos
	end
	parts[#parts + 1] = old:sub(pos + 1)
	return table.concat(parts), target
end

--- `text` without every occurrence of the token `token_text` that would show: one on a
-- line that is no task, or behind another task's token. When the line above it is the
-- head of the token's task — it starts with `head`, the task line up to its body, and
-- has no token — a line break cut the token off, and it goes back there. `caret` (an
-- offset in `text`, or nil) is moved along.
local function unexposed(text, caret, token_text, head)
	local init = 1
	while true do
		local s, e = text:find(token_text, init, true)
		if not s then
			return text, caret
		end
		local from = s
		while from > 1 and text:byte(from - 1) ~= 10 do
			from = from - 1
		end
		local to = text:find("\n", e, true) or (#text + 1)
		local hidden = M.token(text:sub(from, to - 1))
		if hidden and hidden.stop == e - from + 1 then
			init = e + 1
		else
			local start = s - 1
			local blank = text:byte(start)
			if start >= from and (blank == 32 or blank == 9) then
				start = start - 1
			end
			text = text:sub(1, start) .. text:sub(e + 1)
			if caret and caret >= e then
				caret = caret - (e - start)
			elseif caret and caret > start then
				caret = start
			end
			init = start + 1
			local above = from > 1 and text:sub(1, from - 2):match("[^\n]*$")
			if above and above:sub(1, #head) == head and M.body_start(above) and not M.token(above) then
				local at = from - 2
				local back = " " .. token_text
				text = text:sub(1, at) .. back .. text:sub(at + 1)
				if caret and caret > at then
					caret = caret + #back
				end
				init = init + #back
			end
		end
	end
end

--- Repairs an edit that damaged a hidden token or would bring one out of hiding. `old`
-- and `new` are the text of the changed lines before and after it. Returns the text to
-- have instead and the offset in it at which the cursor belongs (nil: where it is), or
-- nil when the edit is fine as it is:
--
-- * a deletion or replacement is cut around the token it would damage; a change inside
--   a token is undone;
-- * text inserted inside or right behind a token goes in front of it;
-- * a line break at the token goes behind it: the token stays on its line;
-- * a token left on a line that is no task any more, or behind another task's token,
--   would only show: it goes back to the head of its task when a line break cut it off,
--   and goes with the edit otherwise.
--
-- With `keep` — the edit left the number of lines as it was, so the task's line is still
-- there — a token is not given up while its line is a task. Without it, a deletion that
-- takes the task's whole body takes the token too: that is how lines are removed and
-- joined.
---@param old string
---@param new string
---@param keep boolean|nil
---@return string|nil, integer|nil
function M.repair(old, new, keep)
	if old == new then
		return nil
	end
	local lines = scan(old)
	local fixed, caret
	-- One line that is no task any more cannot hide a token: there is nothing to protect
	-- on it, and reading the edit as damage would put the token back only to drop it.
	local prose = not (old .. new):find("\n", 1, true) and not M.body_start(new)
	if not prose then
		fixed, caret = mend(lines, old, new, keep)
	end
	local text = fixed or new
	for _, line in ipairs(lines) do
		if line.token then
			text, caret = unexposed(text, caret, line.token.text, old:sub(line.from + 1, line.token.body))
		end
	end
	if text == new then
		return nil
	end
	return text, caret
end

--- Where a cursor at byte column `col` belongs on `line`, given the column it came from
-- on the same line (`prev`, or nil). In insert mode (`insert` true) the cursor is between
-- characters and never rests inside or behind the token; otherwise it is on a character
-- and never on a hidden one. Moving right from the token's edge passes the token when
-- text follows it. Returns nil when `col` is fine.
---@param line string
---@param col integer
---@param prev integer|nil
---@param insert boolean
---@return integer|nil
function M.settle(line, col, prev, insert)
	local token = M.token(line)
	if not token then
		return nil
	end
	local edge = token.start
	if not insert then
		-- The last visible character before the token.
		edge = math.max(token.start - 1, 0)
		while edge > 0 and continuation(line:byte(edge + 1)) do
			edge = edge - 1
		end
		if col < token.start or col >= token.stop then
			return nil
		end
	elseif col <= token.start or col > token.stop then
		return nil
	end
	if prev == edge and token.stop < #line and col < token.stop then
		local past = token.stop
		if insert then
			past = past + 1
			while continuation(line:byte(past + 1)) do
				past = past + 1
			end
		end
		return past
	end
	return edge
end

return M
