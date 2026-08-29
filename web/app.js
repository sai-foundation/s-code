//#region generated/api/client.ts
/**
* Versioned same-origin daemon client. Callers that do not supply a generated
* response type receive `unknown`; credentials are always the HttpOnly cookie.
*/
async function requestEndpoint(path, options = {}) {
	const { allowDisconnected: _allowDisconnected, ...requestOptions } = options;
	const response = await fetch(path, {
		...requestOptions,
		cache: "no-store",
		credentials: "same-origin",
		referrerPolicy: "no-referrer",
		headers: {
			"x-opencoding-csrf": "1",
			...requestOptions.body ? { "content-type": "application/json" } : {},
			...requestOptions.headers || {}
		}
	});
	if (!response.ok) {
		let detail = `${response.status}`;
		const contentType = response.headers.get("content-type") || "";
		try {
			if (contentType.includes("application/json")) {
				const payload = await response.json();
				if (typeof payload.detail === "string") detail = payload.detail;
			} else {
				const text = (await response.text()).trim();
				if (text) detail = text.slice(0, 4096);
			}
		} catch {}
		throw new Error(detail);
	}
	if (response.status === 204) return void 0;
	return await response.json();
}
var requestJson = requestEndpoint;
//#endregion
//#region src/models/runtime.ts
function record(value, label) {
	if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${label} must be an object`);
	return value;
}
function string(value, label) {
	if (typeof value !== "string" || value.length === 0) throw new Error(`${label} must be a non-empty string`);
	return value;
}
function optionalString(value, label) {
	if (value == null) return null;
	return string(value, label);
}
function integer(value, label) {
	if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) throw new Error(`${label} must be a non-negative integer`);
	return value;
}
function finiteNonNegativeNumber(value, label) {
	if (typeof value !== "number" || !Number.isFinite(value) || value < 0) throw new Error(`${label} must be a finite non-negative number`);
	return value;
}
function validateMcpProgress(value, label) {
	const progress = record(value, label);
	const completed = finiteNonNegativeNumber(progress.progress, `${label}.progress`);
	const total = progress.total == null ? null : finiteNonNegativeNumber(progress.total, `${label}.total`);
	if (total != null && completed > total) throw new Error(`${label}.progress cannot exceed total`);
	if (progress.message != null && typeof progress.message !== "string") throw new Error(`${label}.message must be a string or null`);
}
function parseClientEvent(value) {
	const event = record(value, "client event");
	string(event.id, "client event.id");
	integer(event.sequence, "client event.sequence");
	string(event.timestamp, "client event.timestamp");
	string(event.type, "client event.type");
	integer(event.payload_version, "client event.payload_version");
	optionalString(event.session_id, "client event.session_id");
	optionalString(event.turn_id, "client event.turn_id");
	optionalString(event.item_id, "client event.item_id");
	optionalString(event.request_id, "client event.request_id");
	record(event.payload, "client event.payload");
	if (event.notification != null) {
		const notification = record(event.notification, "client event.notification");
		const type = string(notification.type, "client event.notification.type");
		if (type === "mcp_progress_changed") {
			validateMcpProgress(notification, "client event.notification");
			string(notification.item_id, "client event.notification.item_id");
			string(notification.server, "client event.notification.server");
			string(notification.tool, "client event.notification.tool");
		}
		if (type === "reasoning_summary_delta") {
			string(notification.item_id, "client event.notification.item_id");
			if (typeof notification.delta !== "string") throw new Error("client event.notification.delta must be a string");
		}
	}
	return value;
}
function parseTranscriptItem(value, sessionId) {
	const item = record(value, "transcript item");
	string(item.id, "transcript item.id");
	if (string(item.session_id, "transcript item.session_id") !== sessionId) throw new Error("transcript item belongs to a different session");
	string(item.turn_id, "transcript item.turn_id");
	string(item.kind, "transcript item.kind");
	string(item.status, "transcript item.status");
	integer(item.revision, "transcript item.revision");
	const content = record(item.content, "transcript item.content");
	if (content.type === "mcp_call" && content.progress != null) validateMcpProgress(content.progress, "transcript item.content.progress");
	return value;
}
function parseTranscriptSnapshot(value) {
	const snapshot = record(value, "transcript snapshot");
	string(snapshot.protocol_version, "transcript snapshot.protocol_version");
	integer(snapshot.snapshot_revision, "transcript snapshot.snapshot_revision");
	integer(snapshot.cursor, "transcript snapshot.cursor");
	const sessionId = string(record(snapshot.session, "transcript snapshot.session").id, "transcript snapshot.session.id");
	if (!Array.isArray(snapshot.turns)) throw new Error("transcript snapshot.turns must be an array");
	const turnIds = new Set(snapshot.turns.map((value) => {
		const turn = record(value, "transcript turn");
		if (string(turn.session_id, "transcript turn.session_id") !== sessionId) throw new Error("transcript turn belongs to a different session");
		return string(turn.id, "transcript turn.id");
	}));
	if (!Array.isArray(snapshot.items)) throw new Error("transcript snapshot.items must be an array");
	const itemIds = /* @__PURE__ */ new Set();
	for (const value of snapshot.items) {
		const item = parseTranscriptItem(value, sessionId);
		if (!turnIds.has(item.turn_id)) throw new Error("transcript item references an unavailable turn");
		if (itemIds.has(item.id)) throw new Error("transcript snapshot contains a duplicate item");
		itemIds.add(item.id);
	}
	if (!Array.isArray(snapshot.pending_requests) || !Array.isArray(snapshot.pending_questions) || !Array.isArray(snapshot.pending_inputs) || !Array.isArray(snapshot.attachments) || !Array.isArray(snapshot.artifacts)) throw new Error("transcript snapshot request, question, input, attachment, and artifact lists must be arrays");
	const itemCount = snapshot.item_count == null ? snapshot.items.length : integer(snapshot.item_count, "transcript snapshot.item_count");
	const nextCursor = optionalString(snapshot.next_cursor, "transcript snapshot.next_cursor");
	const usage = snapshot.usage == null ? {
		input_tokens: 0,
		output_tokens: 0,
		total_tokens: 0,
		model_calls: 0,
		tool_calls: 0,
		turns: 0
	} : record(snapshot.usage, "transcript snapshot.usage");
	for (const field of [
		"input_tokens",
		"output_tokens",
		"total_tokens",
		"model_calls",
		"tool_calls",
		"turns"
	]) integer(usage[field], `transcript snapshot.usage.${field}`);
	if (usage.total_tokens !== Number(usage.input_tokens) + Number(usage.output_tokens)) throw new Error("transcript snapshot.usage total does not match input and output tokens");
	return {
		...value,
		item_count: itemCount,
		next_cursor: nextCursor,
		usage
	};
}
//#endregion
//#region src/state/transcript.ts
function emptyTranscriptProjection() {
	return {
		activeSessionId: null,
		cursor: 0,
		snapshotRevision: 0,
		itemRevisions: /* @__PURE__ */ new Map(),
		itemByteLengths: /* @__PURE__ */ new Map()
	};
}
function selectTranscriptSession(state, sessionId) {
	if (state.activeSessionId === sessionId) return state;
	return {
		activeSessionId: sessionId,
		cursor: state.cursor,
		snapshotRevision: state.snapshotRevision,
		itemRevisions: /* @__PURE__ */ new Map(),
		itemByteLengths: /* @__PURE__ */ new Map()
	};
}
function isTranscriptSnapshotStale(state, snapshot) {
	return snapshot.snapshot_revision < state.snapshotRevision;
}
function applyTranscriptSnapshot(state, snapshot) {
	if (isTranscriptSnapshotStale(state, snapshot)) return state;
	return {
		activeSessionId: snapshot.session.id,
		cursor: Math.max(state.cursor, snapshot.cursor),
		snapshotRevision: Math.max(state.snapshotRevision, snapshot.snapshot_revision),
		itemRevisions: new Map(snapshot.items.map((item) => [item.id, item.revision])),
		itemByteLengths: new Map(snapshot.items.flatMap((item) => {
			if (!item.content || item.content.type !== "message" || item.content.role !== "assistant" || typeof item.content.content !== "string") return [];
			return [[item.id, new TextEncoder().encode(item.content.content).byteLength]];
		}))
	};
}
function reduceClientEvent(state, event) {
	if (event.sequence <= state.cursor) return {
		state,
		accepted: false,
		visible: false,
		gap: null,
		appendGap: null
	};
	if (state.cursor > 0 && event.sequence !== state.cursor + 1) return {
		state,
		accepted: false,
		visible: false,
		gap: {
			expected: state.cursor + 1,
			received: event.sequence
		},
		appendGap: null
	};
	const visible = event.session_id == null || event.session_id === state.activeSessionId;
	const byteLengths = new Map(state.itemByteLengths);
	if (visible && event.notification?.type === "agent_message_delta") {
		const itemId = event.notification.item_id;
		const expected = byteLengths.get(itemId) || 0;
		const received = event.notification.byte_offset;
		if (received != null && received !== expected) return {
			state,
			accepted: false,
			visible: false,
			gap: null,
			appendGap: {
				itemId,
				expected,
				received
			}
		};
		byteLengths.set(itemId, expected + new TextEncoder().encode(event.notification.delta).byteLength);
	}
	const revisions = new Map(state.itemRevisions);
	if (visible && event.item_id) revisions.set(event.item_id, (revisions.get(event.item_id) || 0) + 1);
	return {
		state: {
			activeSessionId: state.activeSessionId,
			cursor: event.sequence,
			snapshotRevision: Math.max(state.snapshotRevision, event.sequence),
			itemRevisions: revisions,
			itemByteLengths: byteLengths
		},
		accepted: true,
		visible,
		gap: null,
		appendGap: null
	};
}
//#endregion
//#region src/router.ts
function parseRoute(pathname) {
	if (pathname === "/team") return {
		type: "team",
		section: "overview"
	};
	const teamSection = pathname.match(/^\/team\/([^/]+)$/)?.[1];
	if (teamSection && [
		"work",
		"goals",
		"agents",
		"capacity",
		"ownership",
		"budgets",
		"approvals",
		"outcomes",
		"audit"
	].includes(teamSection)) return {
		type: "team",
		section: teamSection
	};
	if (pathname === "/projects") return { type: "projects" };
	if (pathname === "/artifacts") return { type: "artifacts" };
	if (pathname === "/settings/extensions") return { type: "extensions" };
	const artifact = pathname.match(/^\/artifacts\/([^/]+)$/);
	if (artifact) return {
		type: "artifact",
		artifactId: decodeURIComponent(artifact[1])
	};
	const project = pathname.match(/^\/projects\/([^/]+)$/);
	if (project) return {
		type: "project",
		projectId: decodeURIComponent(project[1])
	};
	const match = pathname.match(/^\/chat\/([^/]+)$/);
	if (match) return {
		type: "session",
		sessionId: decodeURIComponent(match[1])
	};
	return { type: "home" };
}
function teamRoute(section) {
	return section === "overview" ? "/team" : `/team/${section}`;
}
function sessionRoute(sessionId) {
	return sessionId ? `/chat/${encodeURIComponent(sessionId)}` : "/";
}
function projectRoute(projectId) {
	return projectId ? `/projects/${encodeURIComponent(projectId)}` : "/projects";
}
function artifactRoute(artifactId) {
	return artifactId ? `/artifacts/${encodeURIComponent(artifactId)}` : "/artifacts";
}
function extensionsRoute() {
	return "/settings/extensions";
}
//#endregion
//#region src/render/transcript.ts
function transcriptItemIdentity(item) {
	return {
		itemId: item.id,
		turnId: item.turn_id
	};
}
function isMessageItem(item) {
	return (item.kind === "user_message" || item.kind === "agent_message") && item.content.type === "message";
}
function isToolItem(item) {
	return ["tool_call", "mcp_call"].includes(item.content.type) && [
		"tool_call",
		"command_execution",
		"file_read",
		"file_search",
		"file_change",
		"mcp_call",
		"hook",
		"dynamic_tool",
		"diff"
	].includes(item.kind);
}
//#endregion
//#region src/render/highlight.ts
var HighlightClient = class {
	sequence = 0;
	pending = /* @__PURE__ */ new Map();
	worker;
	constructor(workerFactory = () => new Worker("/highlight-worker.js", { name: "opencoding-code-highlight" })) {
		this.worker = typeof Worker === "undefined" ? null : workerFactory();
		this.worker?.addEventListener("message", (event) => {
			const resolve = this.pending.get(event.data.id);
			if (!resolve) return;
			this.pending.delete(event.data.id);
			resolve(Array.isArray(event.data.tokens) ? event.data.tokens : []);
		});
		this.worker?.addEventListener("error", () => {
			for (const resolve of this.pending.values()) resolve([]);
			this.pending.clear();
		});
	}
	request(code, language, resolve) {
		if (!this.worker || code.length < 32 || code.length > 2e5 || [
			"text",
			"plaintext",
			"diff"
		].includes(language.toLowerCase())) return false;
		const id = ++this.sequence;
		this.pending.set(id, resolve);
		this.worker.postMessage({
			id,
			code,
			language
		});
		return true;
	}
	dispose() {
		for (const resolve of this.pending.values()) resolve([]);
		this.pending.clear();
		this.worker?.terminate();
	}
};
//#endregion
//#region src/render/markdown.ts
function createMarkdownRenderer({ copyText, highlight }) {
	function safeUrl(value, image = false) {
		const raw = String(value).trim().replace(/^<|>$/g, "");
		try {
			const parsed = new URL(raw, window.location.href);
			return (image ? ["http:", "https:"] : [
				"http:",
				"https:",
				"mailto:"
			]).includes(parsed.protocol) ? raw : null;
		} catch {
			return null;
		}
	}
	function appendInline(target, text) {
		const source = String(text);
		const token = /\\[\\`*_[\]{}()#+.!|>~-]|`+[^`\n]+`+|!\[[^\]\n]*\]\([^) \n]+(?:\s+["'][^"'\n]*["'])?\)|\[[^\]\n]+\]\([^) \n]+(?:\s+["'][^"'\n]*["'])?\)|\*\*[^*\n]+\*\*|__[^_\n]+__|~~[^~\n]+~~|\*[^*\n]+\*|_[^_\n]+_|<(?:https?:\/\/[^ >]+|[^ <@]+@[^ >@]+)>| {2,}\n/g;
		let cursor = 0;
		let match;
		while ((match = token.exec(source)) !== null) {
			if (match.index > cursor) target.append(document.createTextNode(source.slice(cursor, match.index)));
			const value = match[0];
			if (value.startsWith("\\")) target.append(document.createTextNode(value.slice(1)));
			else if (/^`/.test(value)) {
				const ticks = value.match(/^`+/)?.[0].length ?? 1;
				const code = document.createElement("code");
				code.textContent = value.slice(ticks, -ticks);
				target.append(code);
			} else if (value.startsWith("![")) {
				const parts = value.match(/^!\[([^\]]*)\]\(([^ )]+)(?:\s+["']([^"']*)["'])?\)$/);
				const url = parts && safeUrl(parts[2], true);
				if (!url) target.append(document.createTextNode(value));
				else {
					const image = document.createElement("img");
					image.src = url;
					image.alt = parts[1];
					if (parts[3]) image.title = parts[3];
					image.loading = "lazy";
					image.referrerPolicy = "no-referrer";
					target.append(image);
				}
			} else if (value.startsWith("[")) {
				const parts = value.match(/^\[([^\]]+)\]\(([^ )]+)(?:\s+["']([^"']*)["'])?\)$/);
				const url = parts && safeUrl(parts[2]);
				if (!url) target.append(document.createTextNode(value));
				else {
					const link = document.createElement("a");
					link.href = url;
					if (parts[3]) link.title = parts[3];
					link.rel = "noreferrer noopener";
					appendInline(link, parts[1]);
					target.append(link);
				}
			} else if (value.startsWith("**") || value.startsWith("__")) {
				const strong = document.createElement("strong");
				appendInline(strong, value.slice(2, -2));
				target.append(strong);
			} else if (value.startsWith("~~")) {
				const deleted = document.createElement("del");
				appendInline(deleted, value.slice(2, -2));
				target.append(deleted);
			} else if (value === "  \n") target.append(document.createElement("br"));
			else if (value.startsWith("<")) {
				const raw = value.slice(1, -1);
				const url = safeUrl(raw.includes("@") && !raw.includes("://") ? `mailto:${raw}` : raw);
				if (!url) target.append(document.createTextNode(value));
				else {
					const link = document.createElement("a");
					link.href = url;
					link.rel = "noreferrer noopener";
					link.textContent = raw;
					target.append(link);
				}
			} else {
				const emphasis = document.createElement("em");
				appendInline(emphasis, value.slice(1, -1));
				target.append(emphasis);
			}
			cursor = match.index + value.length;
		}
		if (cursor < source.length) target.append(document.createTextNode(source.slice(cursor)));
	}
	function listMarker(line) {
		const match = String(line).match(/^(\s*)([-+*]|\d+[.)])\s+(.+)$/);
		if (!match) return null;
		return {
			indent: match[1].replace(/\t/g, "    ").length,
			ordered: /^\d/.test(match[2]),
			start: /^\d/.test(match[2]) ? Number.parseInt(match[2], 10) : null,
			content: match[3]
		};
	}
	function appendList(target, lines, start, indent) {
		const first = listMarker(lines[start]);
		if (!first) return { index: start + 1 };
		const list = document.createElement(first.ordered ? "ol" : "ul");
		if (first.ordered && first.start !== null && first.start !== 1) list.start = first.start;
		let index = start;
		while (index < lines.length) {
			const marker = listMarker(lines[index]);
			if (!marker || marker.indent !== indent || marker.ordered !== first.ordered) break;
			const item = document.createElement("li");
			const task = marker.content.match(/^\[([ xX])\]\s+(.+)$/);
			if (task) {
				item.classList.add("task-list-item");
				list.classList.add("task-list");
				const checkbox = document.createElement("input");
				checkbox.type = "checkbox";
				checkbox.checked = task[1].toLowerCase() === "x";
				checkbox.disabled = true;
				checkbox.setAttribute("aria-label", checkbox.checked ? "Completed task" : "Incomplete task");
				item.append(checkbox);
				appendInline(item, task[2]);
			} else appendInline(item, marker.content);
			index += 1;
			while (index < lines.length) {
				const nested = listMarker(lines[index]);
				if (nested && nested.indent > indent) {
					index = appendList(item, lines, index, nested.indent).index;
					continue;
				}
				if (nested || !lines[index].trim() || !/^\s+/.test(lines[index])) break;
				item.append(document.createElement("br"));
				appendInline(item, lines[index].trim());
				index += 1;
			}
			list.append(item);
			if (!lines[index]?.trim()) break;
		}
		target.append(list);
		return { index };
	}
	function splitTableRow(line) {
		const trimmed = String(line).trim().replace(/^\|/, "").replace(/\|$/, "");
		const cells = [];
		let value = "";
		let escaped = false;
		for (const character of trimmed) if (escaped) {
			value += character;
			escaped = false;
		} else if (character === "\\") escaped = true;
		else if (character === "|") {
			cells.push(value.trim());
			value = "";
		} else value += character;
		cells.push(value.trim());
		return cells;
	}
	function tableAlignments(line) {
		const cells = splitTableRow(line);
		if (!cells.length || cells.some((cell) => !/^:?-{3,}:?$/.test(cell))) return null;
		return cells.map((cell) => cell.startsWith(":") && cell.endsWith(":") ? "center" : cell.endsWith(":") ? "right" : cell.startsWith(":") ? "left" : null);
	}
	function appendTable(target, lines, start, alignments) {
		const table = document.createElement("table");
		const head = document.createElement("thead");
		const headRow = document.createElement("tr");
		splitTableRow(lines[start]).forEach((content, index) => {
			const cell = document.createElement("th");
			if (alignments[index]) cell.style.textAlign = alignments[index];
			appendInline(cell, content);
			headRow.append(cell);
		});
		head.append(headRow);
		table.append(head);
		const body = document.createElement("tbody");
		let index = start + 2;
		while (index < lines.length && lines[index].includes("|") && lines[index].trim()) {
			const row = document.createElement("tr");
			splitTableRow(lines[index]).forEach((content, cellIndex) => {
				const cell = document.createElement("td");
				if (alignments[cellIndex]) cell.style.textAlign = alignments[cellIndex];
				appendInline(cell, content);
				row.append(cell);
			});
			body.append(row);
			index += 1;
		}
		table.append(body);
		target.append(table);
		return index;
	}
	function appendBlocks(target, text) {
		const lines = String(text).replace(/\r\n?/g, "\n").split("\n");
		let index = 0;
		const beginsBlock = (position) => {
			const line = lines[position] || "";
			return !line.trim() || /^( {0,3})(#{1,6})\s+/.test(line) || /^( {0,3})(`{3,}|~{3,})/.test(line) || /^( {0,3})>\s?/.test(line) || /^( {0,3})([-*_])(?:\s*\2){2,}\s*$/.test(line) || Boolean(listMarker(line)) || line.includes("|") && Boolean(tableAlignments(lines[position + 1] || ""));
		};
		while (index < lines.length) {
			const line = lines[index];
			if (!line.trim()) {
				index += 1;
				continue;
			}
			const fence = line.match(/^( {0,3})(`{3,}|~{3,})\s*([\w.+-]*)\s*$/);
			if (fence) {
				const marker = fence[2][0];
				const minimum = fence[2].length;
				const code = [];
				index += 1;
				while (index < lines.length && !new RegExp(`^ {0,3}${marker}{${minimum},}\\s*$`).test(lines[index])) {
					code.push(lines[index]);
					index += 1;
				}
				if (index < lines.length) index += 1;
				target.append(createCodeBlock(code.join("\n"), fence[3] || "text"));
				continue;
			}
			const heading = line.match(/^( {0,3})(#{1,6})\s+(.+?)\s*#*\s*$/);
			if (heading) {
				const element = document.createElement(`h${Math.min(6, heading[2].length + 1)}`);
				appendInline(element, heading[3]);
				target.append(element);
				index += 1;
				continue;
			}
			if (/^( {0,3})([-*_])(?:\s*\2){2,}\s*$/.test(line)) {
				target.append(document.createElement("hr"));
				index += 1;
				continue;
			}
			if (/^( {0,3})>\s?/.test(line)) {
				const quoteLines = [];
				while (index < lines.length && /^( {0,3})>\s?/.test(lines[index])) {
					quoteLines.push(lines[index].replace(/^( {0,3})>\s?/, ""));
					index += 1;
				}
				const quote = document.createElement("blockquote");
				appendBlocks(quote, quoteLines.join("\n"));
				target.append(quote);
				continue;
			}
			const marker = listMarker(line);
			if (marker) {
				index = appendList(target, lines, index, marker.indent).index;
				continue;
			}
			const alignments = line.includes("|") ? tableAlignments(lines[index + 1] || "") : null;
			if (alignments) {
				index = appendTable(target, lines, index, alignments);
				continue;
			}
			const paragraph = [];
			while (index < lines.length && (paragraph.length === 0 || !beginsBlock(index))) {
				paragraph.push(lines[index]);
				index += 1;
			}
			const element = document.createElement("p");
			appendInline(element, paragraph.join("\n"));
			target.append(element);
		}
	}
	function createCodeBlock(code, language = "text") {
		const wrapper = document.createElement("section");
		wrapper.className = "code-block";
		wrapper.setAttribute("aria-label", `${language || "text"} code`);
		const header = document.createElement("div");
		header.className = "code-header";
		const label = document.createElement("span");
		label.textContent = language || "text";
		const copy = document.createElement("button");
		copy.type = "button";
		copy.className = "code-copy";
		copy.textContent = "Copy";
		copy.setAttribute("aria-label", `Copy ${language || "text"} code`);
		copy.addEventListener("click", () => void copyText(code, "Code copied"));
		const pre = document.createElement("pre");
		const codeElement = document.createElement("code");
		codeElement.textContent = code;
		highlight(code, language, (tokens) => {
			if (!codeElement.isConnected || !tokens.length) return;
			const fragment = document.createDocumentFragment();
			for (const token of tokens) if (token.kind === "plain") fragment.append(document.createTextNode(token.text));
			else {
				const span = document.createElement("span");
				span.className = `syntax-${token.kind}`;
				span.textContent = token.text;
				fragment.append(span);
			}
			codeElement.replaceChildren(fragment);
		});
		pre.append(codeElement);
		header.append(label, copy);
		wrapper.append(header, pre);
		return wrapper;
	}
	function renderMessageContent(item, content) {
		let body = item.querySelector(".message-body");
		if (!body) {
			body = document.createElement("div");
			body.className = "message-body";
			item.append(body);
		}
		body.replaceChildren();
		if (typeof content !== "string") {
			body.append(createCodeBlock(JSON.stringify(content, null, 2), "json"));
			return;
		}
		appendBlocks(body, content);
	}
	return {
		appendBlocks,
		createCodeBlock,
		renderMessageContent,
		safeUrl
	};
}
//#endregion
//#region src/pages/extensions.ts
function createExtensionsPage(context) {
	const { lookup: $, api, state, scope, requestAction, toast, copyText, closeDrawers, closeUserMenu, catalogQuery, routePath } = context;
	let extensionEntries = [];
	let selectedExtensionId = null;
	function extensionMcpServerId(extension) {
		return extension.id.startsWith("mcp:") ? extension.id.slice(4) : extension.id;
	}
	async function readMcpResource(extension, uri, output) {
		const serverId = extensionMcpServerId(extension);
		const query = catalogQuery();
		query.set("uri", uri);
		output.replaceChildren();
		output.textContent = "Reading resource…";
		const resource = await api(`/v1/mcp/${encodeURIComponent(serverId)}/resources/read?${query}`);
		if (selectedExtensionId !== extension.id) return;
		output.replaceChildren();
		if (!resource.contents.length) {
			output.textContent = "The server returned no content.";
			return;
		}
		resource.contents.forEach((content) => {
			const article = document.createElement("article");
			article.className = "mcp-resource-content";
			const heading = document.createElement("strong");
			heading.textContent = `${content.uri} · ${content.mime_type || "unknown type"}`;
			const body = document.createElement("pre");
			body.textContent = content.text ?? `[base64 blob · ${content.blob_base64?.length || 0} characters]`;
			article.append(heading, body);
			output.append(article);
		});
	}
	async function browseMcpResources(extension, cursor = null) {
		const target = $("extension-detail");
		const serverId = extensionMcpServerId(extension);
		const query = catalogQuery();
		if (cursor) query.set("cursor", cursor);
		let browser = target.querySelector(".mcp-resource-browser");
		if (!browser) {
			browser = document.createElement("section");
			browser.className = "mcp-resource-browser";
			target.append(browser);
		}
		browser.replaceChildren();
		browser.textContent = "Loading MCP resources…";
		const [page, templates] = await Promise.all([api(`/v1/mcp/${encodeURIComponent(serverId)}/resources?${query}`), cursor ? Promise.resolve({
			resource_templates: [],
			next_cursor: null
		}) : api(`/v1/mcp/${encodeURIComponent(serverId)}/resource-templates?${catalogQuery()}`)]);
		if (selectedExtensionId !== extension.id) return;
		browser.replaceChildren();
		const heading = document.createElement("strong");
		heading.textContent = `Resources · ${page.resources.length}`;
		browser.append(heading);
		if (!page.resources.length) {
			const empty = document.createElement("p");
			empty.textContent = "This server exposes no resources on this page.";
			browser.append(empty);
		}
		const output = document.createElement("div");
		output.className = "mcp-resource-output";
		page.resources.forEach((resource) => {
			const row = document.createElement("button");
			row.type = "button";
			row.className = "mcp-resource-row";
			const name = document.createElement("strong");
			name.textContent = resource.name;
			const uri = document.createElement("span");
			uri.textContent = resource.uri;
			const detail = document.createElement("small");
			detail.textContent = [resource.mime_type, resource.description].filter(Boolean).join(" · ");
			row.append(name, uri, detail);
			row.addEventListener("click", () => {
				readMcpResource(extension, resource.uri, output).catch((error) => toast(error.message));
			});
			browser.append(row);
		});
		if (page.next_cursor) {
			const next = document.createElement("button");
			next.type = "button";
			next.textContent = "Next resource page";
			next.addEventListener("click", () => {
				browseMcpResources(extension, page.next_cursor).catch((error) => toast(error.message));
			});
			browser.append(next);
		}
		if (templates.resource_templates.length) {
			const templateHeading = document.createElement("strong");
			templateHeading.textContent = `Templates · ${templates.resource_templates.length}`;
			browser.append(templateHeading);
			templates.resource_templates.forEach((template) => {
				const row = document.createElement("article");
				row.className = "mcp-resource-template";
				const name = document.createElement("strong");
				name.textContent = template.name;
				const uri = document.createElement("span");
				uri.textContent = template.uri_template;
				const detail = document.createElement("small");
				detail.textContent = [template.mime_type, template.description].filter(Boolean).join(" · ");
				row.append(name, uri, detail);
				browser.append(row);
			});
		}
		browser.append(output);
	}
	function showExtensionDetail(extension) {
		selectedExtensionId = extension.id;
		renderExtensionList();
		const target = $("extension-detail");
		target.replaceChildren();
		const title = document.createElement("h3");
		title.textContent = extension.name;
		const description = document.createElement("p");
		description.textContent = extension.description;
		const facts = document.createElement("dl");
		facts.className = "extension-facts";
		const addFact = (label, value) => {
			const term = document.createElement("dt");
			term.textContent = label;
			const detail = document.createElement("dd");
			detail.textContent = value;
			facts.append(term, detail);
		};
		addFact("Kind", extension.kind.replaceAll("_", " "));
		addFact("Status", extension.status.replaceAll("_", " "));
		addFact("Source", extension.source_uri);
		addFact("Trust", extension.trust.replaceAll("_", " "));
		addFact("Signature", extension.signature_verified ? "Verified" : "Not an Enterprise-signed package");
		addFact("Allowlist", extension.allowlisted ? "Allowed" : "Not allowed");
		addFact("Authentication", extension.oauth_supported ? extension.authenticated ? "OAuth connected" : "OAuth required" : "No OAuth flow");
		if (extension.publisher) addFact("Publisher", extension.publisher);
		if (extension.version) addFact("Version", extension.version);
		target.append(title, description, facts);
		if (extension.locked_reason) {
			const lock = document.createElement("p");
			lock.className = "extension-lock";
			lock.textContent = `Managed setting · ${extension.locked_reason}`;
			target.append(lock);
		}
		const permissionHeading = document.createElement("strong");
		permissionHeading.textContent = `Effective permissions · ${extension.permissions.length}`;
		target.append(permissionHeading);
		const permissions = document.createElement("div");
		permissions.className = "extension-permissions";
		if (!extension.permissions.length) permissions.textContent = "No runtime permissions declared.";
		extension.permissions.forEach((permission) => {
			const row = document.createElement("article");
			row.className = "extension-permission";
			const kind = document.createElement("strong");
			kind.textContent = permission.kind;
			const value = document.createElement("span");
			value.textContent = permission.value;
			const reason = document.createElement("small");
			reason.textContent = permission.reason;
			row.append(kind, value, reason);
			permissions.append(row);
		});
		target.append(permissions);
		const isLocalMcp = extension.kind === "mcp_server" && (extension.source_uri.startsWith("opencoding://extensions/mcp/") || extension.source_uri.startsWith("opencoding://extensions/mcp-http/"));
		const isLocalHook = extension.kind === "hook" && extension.source_uri.startsWith("opencoding://extensions/hooks/");
		const isLocalSkill = extension.kind === "skill" && extension.source_uri.startsWith("file://");
		const isLocalPlugin = extension.kind === "plugin" && extension.id.startsWith("plugin:");
		const actions = document.createElement("div");
		actions.className = "artifact-library-actions";
		if (extension.kind === "mcp_server" && extension.status === "connected" && state.capabilities.has("mcp.resources.v1")) {
			const browse = document.createElement("button");
			browse.type = "button";
			browse.textContent = "Browse resources";
			browse.addEventListener("click", () => {
				browseMcpResources(extension).catch((error) => toast(error.message));
			});
			actions.append(browse);
		}
		if (isLocalMcp && extension.source_uri.startsWith("opencoding://extensions/mcp-http/") && extension.oauth_supported && state.capabilities.has("mcp.oauth.pkce.v1")) {
			const auth = document.createElement("button");
			auth.type = "button";
			auth.textContent = extension.authenticated ? "Log out OAuth" : "Log in with OAuth";
			auth.addEventListener("click", () => {
				(extension.authenticated ? logoutMcpOAuth(extension) : loginMcpOAuth(extension)).catch((error) => toast(error.message));
			});
			actions.append(auth);
		}
		if ((isLocalMcp || isLocalSkill || isLocalHook) && extension.permissions_sha256) {
			if (isLocalSkill) {
				const toggle = document.createElement("button");
				toggle.type = "button";
				const enable = extension.status === "disabled";
				toggle.textContent = enable ? "Enable" : "Disable";
				toggle.addEventListener("click", () => {
					setSkillEnabled(extension, enable).catch((error) => toast(error.message));
				});
				actions.append(toggle);
			}
			const remove = document.createElement("button");
			remove.type = "button";
			remove.className = "danger";
			remove.textContent = "Remove";
			remove.addEventListener("click", () => {
				(isLocalHook ? removeHook(extension) : isLocalSkill ? removeSkill(extension) : removeMcpServer(extension)).catch((error) => toast(error.message));
			});
			actions.append(remove);
		}
		if (isLocalPlugin) {
			const install = document.createElement("button");
			install.type = "button";
			install.textContent = extension.status === "available" ? "Install" : "Review update";
			install.addEventListener("click", () => {
				installOrUpdatePlugin(extension).catch((error) => toast(error.message));
			});
			actions.append(install);
			if (extension.status === "installed" || extension.status === "disabled") {
				const toggle = document.createElement("button");
				toggle.type = "button";
				const enable = extension.status === "disabled";
				toggle.textContent = enable ? "Enable" : "Disable";
				toggle.addEventListener("click", () => {
					setPluginEnabled(extension, enable).catch((error) => toast(error.message));
				});
				const remove = document.createElement("button");
				remove.type = "button";
				remove.className = "danger";
				remove.textContent = "Remove";
				remove.addEventListener("click", () => {
					removePlugin(extension).catch((error) => toast(error.message));
				});
				actions.append(toggle, remove);
			}
		}
		if (actions.childElementCount) target.append(actions);
		if (isLocalPlugin) enrichPluginDetail(extension, target).catch((error) => toast(error.message));
	}
	function renderExtensionList() {
		const kind = $("extension-filter").value;
		const query = $("extension-search").value.trim().toLocaleLowerCase();
		const visible = extensionEntries.filter((extension) => (kind === "all" || extension.kind === kind) && (!query || [
			extension.name,
			extension.description,
			extension.source_uri,
			extension.publisher || ""
		].some((value) => value.toLocaleLowerCase().includes(query))));
		const list = $("extension-list");
		list.replaceChildren();
		list.classList.toggle("empty", !visible.length);
		$("extension-count").textContent = `${visible.length} extension${visible.length === 1 ? "" : "s"}`;
		if (!visible.length) list.textContent = extensionEntries.length ? "No extensions match this kind." : "No extensions configured.";
		visible.forEach((extension) => {
			const button = document.createElement("button");
			button.type = "button";
			button.className = "extension-row";
			button.classList.toggle("active", extension.id === selectedExtensionId);
			const heading = document.createElement("span");
			const name = document.createElement("strong");
			name.textContent = extension.name;
			const status = document.createElement("span");
			status.className = "extension-status";
			status.textContent = extension.status.replaceAll("_", " ");
			heading.append(name, status);
			const kind = document.createElement("small");
			kind.textContent = `${extension.kind.replaceAll("_", " ")} · ${extension.trust.replaceAll("_", " ")}`;
			const source = document.createElement("small");
			source.textContent = extension.source_uri;
			button.append(heading, kind, source);
			button.addEventListener("click", () => showExtensionDetail(extension));
			list.append(button);
		});
	}
	async function loadExtensionCatalog() {
		if (!state.connected) {
			extensionEntries = [];
			renderExtensionList();
			$("extension-detail").textContent = "Connect to inspect extensions.";
			return;
		}
		extensionEntries = await api(`/v1/extensions?${catalogQuery()}`);
		const selected = extensionEntries.find((extension) => extension.id === selectedExtensionId) || extensionEntries[0] || null;
		selectedExtensionId = selected?.id || null;
		renderExtensionList();
		if (selected) showExtensionDetail(selected);
		else {
			const empty = document.createElement("p");
			empty.className = "empty";
			empty.textContent = "No extensions configured.";
			$("extension-detail").replaceChildren(empty);
		}
	}
	function parseEnvironmentHandles(value) {
		const handles = {};
		value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean).forEach((line) => {
			const separator = line.indexOf("=");
			if (separator <= 0 || separator === line.length - 1) throw new Error("Environment handles must use NAME=ENVIRONMENT_VARIABLE, one per line");
			const name = line.slice(0, separator).trim();
			const handle = line.slice(separator + 1).trim();
			if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(handle)) throw new Error("Environment names and handles must be valid variable names");
			handles[name] = handle;
		});
		return handles;
	}
	function parseHeaderHandles(value) {
		const handles = {};
		value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean).forEach((line) => {
			const separator = line.indexOf("=");
			if (separator <= 0 || separator === line.length - 1) throw new Error("Header handles must use HEADER=ENVIRONMENT_VARIABLE, one per line");
			const name = line.slice(0, separator).trim();
			const handle = line.slice(separator + 1).trim();
			if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(name) || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(handle)) throw new Error("Header names and environment-variable handles are invalid");
			handles[name] = handle;
		});
		return handles;
	}
	async function addMcpServer() {
		if (!state.connected) {
			toast("Connect to Opencoding before installing an MCP server.");
			return;
		}
		const values = await requestAction({
			eyebrow: "Community extension",
			title: "Add an MCP server",
			description: "Choose local stdio, header-authenticated HTTP, or OAuth HTTP. Secrets stay in the daemon; enter only environment-variable handles here.",
			confirm: "Preview permissions",
			fields: [
				{
					name: "id",
					label: "Server ID",
					placeholder: "local-tools",
					required: true,
					maxlength: 64
				},
				{
					name: "transport",
					label: "Transport",
					options: [
						["stdio", "Local stdio"],
						["http", "Streamable HTTP · header handles"],
						["http_oauth", "Streamable HTTP · OAuth 2.1 PKCE"]
					]
				},
				{
					name: "program",
					label: "Absolute executable path (stdio)",
					placeholder: "/usr/local/bin/example-mcp"
				},
				{
					name: "args",
					label: "Arguments (one per line)",
					multiline: true,
					placeholder: "--stdio"
				},
				{
					name: "environment",
					label: "Environment handles (NAME=VARIABLE)",
					multiline: true,
					placeholder: "API_TOKEN=EXAMPLE_API_TOKEN"
				},
				{
					name: "endpoint",
					label: "Endpoint (Streamable HTTP)",
					placeholder: "https://mcp.example.com/mcp"
				},
				{
					name: "headers",
					label: "Header handles (HEADER=VARIABLE)",
					multiline: true,
					placeholder: "Authorization=MCP_ACCESS_TOKEN"
				},
				{
					name: "oauth_client_id",
					label: "OAuth public client ID (optional)",
					placeholder: "Leave empty for dynamic registration"
				},
				{
					name: "oauth_scopes",
					label: "OAuth scopes (one per line, optional)",
					multiline: true,
					placeholder: "mcp:tools\nmcp:resources"
				},
				{
					name: "timeout",
					label: "Timeout (milliseconds)",
					type: "number",
					min: 100,
					value: 3e4,
					required: true
				}
			]
		});
		if (!values) return;
		const http = values.transport === "http" || values.transport === "http_oauth";
		const oauth = values.transport === "http_oauth";
		const server = http ? {
			id: values.id.trim(),
			endpoint: values.endpoint.trim(),
			header_handles: oauth ? {} : parseHeaderHandles(values.headers),
			oauth: oauth ? {
				client_id: values.oauth_client_id.trim() || null,
				scopes: values.oauth_scopes.split(/\r?\n/).map((scope) => scope.trim()).filter(Boolean)
			} : null,
			timeout_ms: Number(values.timeout)
		} : {
			id: values.id.trim(),
			program: values.program.trim(),
			args: values.args.split(/\r?\n/).map((line) => line.trim()).filter(Boolean),
			environment_handles: parseEnvironmentHandles(values.environment),
			timeout_ms: Number(values.timeout)
		};
		const endpoint = http ? "/v1/extensions/mcp-http" : "/v1/extensions/mcp";
		const preview = await api(`${endpoint}/preview`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				server
			})
		});
		if (!await requestAction({
			eyebrow: "Permission review",
			title: `Install ${preview.descriptor.name}?`,
			description: "Review every effective permission. The server starts only after Opencoding restarts.",
			details: [
				...preview.descriptor.permissions.map((permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`),
				`Scope: Team ${scope().team_id}`,
				`Source: ${preview.descriptor.source_uri}`,
				`Restart required: ${preview.requires_restart ? "yes" : "no"}`
			],
			confirm: "Install MCP server"
		})) return;
		await api(`${endpoint}/install`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				server,
				confirmation: {
					confirmed: true,
					permissions_sha256: preview.permissions_sha256
				}
			})
		});
		await loadExtensionCatalog();
		toast(oauth ? "MCP server installed · select it and log in with OAuth" : "MCP server installed · restart Opencoding to connect");
	}
	async function loginMcpOAuth(extension) {
		const serverId = extension.id.replace(/^mcp:/, "");
		const discovery = await api(`/v1/extensions/mcp-http/${encodeURIComponent(serverId)}/oauth/preview`, {
			method: "POST",
			body: JSON.stringify({ scope: scope() })
		});
		if (!await requestAction({
			eyebrow: "MCP OAuth",
			title: `Log in to ${extension.name}?`,
			description: "Opencoding will open the provider in a separate window. Access and refresh tokens remain encrypted in the daemon.",
			details: [
				`Identity provider: ${discovery.authorization_server}`,
				`Login endpoint: ${discovery.authorization_endpoint}`,
				`Token endpoint: ${discovery.token_endpoint}`,
				`Scopes: ${discovery.scopes.join(", ") || "none requested"}`,
				"Callback: local daemon only"
			],
			confirm: "Open OAuth login"
		})) return;
		const launch = await api(`/v1/extensions/mcp-http/${encodeURIComponent(serverId)}/oauth`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				confirmation: {
					confirmed: true,
					permissions_sha256: discovery.permissions_sha256
				}
			})
		});
		if (!window.open(launch.authorization_url, `opencoding-mcp-oauth-${serverId}`, "popup,width=720,height=760,noopener,noreferrer")) {
			await copyText(launch.authorization_url, "OAuth login URL copied");
			toast("The browser blocked the login window · URL copied");
		} else toast("Complete the login in the provider window");
		const expiresAt = Date.parse(launch.expires_at);
		while (Date.now() < expiresAt) {
			await new Promise((resolve) => window.setTimeout(resolve, 1e3));
			if ((await api(`/v1/extensions/mcp-http/${encodeURIComponent(serverId)}/oauth?${catalogQuery()}`)).authenticated) {
				await loadExtensionCatalog();
				toast("OAuth login complete · restart Opencoding to connect");
				return;
			}
		}
		throw new Error("OAuth login expired; start it again");
	}
	async function logoutMcpOAuth(extension) {
		if (!await requestAction({
			eyebrow: "MCP OAuth",
			title: `Log out of ${extension.name}?`,
			description: "The daemon will revoke the token when the provider supports revocation, then remove its encrypted local credential.",
			details: [
				`MCP server: ${extension.id}`,
				`Scope: Team ${scope().team_id}`,
				"Restart required to disconnect the running server"
			],
			confirm: "Log out OAuth",
			danger: true
		})) return;
		const serverId = extension.id.replace(/^mcp:/, "");
		await api(`/v1/extensions/mcp-http/${encodeURIComponent(serverId)}/oauth`, {
			method: "DELETE",
			body: JSON.stringify({ scope: scope() })
		});
		await loadExtensionCatalog();
		toast("OAuth login removed · restart Opencoding to disconnect");
	}
	async function removeMcpServer(extension) {
		if (!extension.permissions_sha256) throw new Error("This MCP installation has no removable permission revision");
		if (!await requestAction({
			eyebrow: "Community extension",
			title: `Remove ${extension.name}?`,
			description: "The persisted installation will be removed. Restart Opencoding to stop the currently connected process and remove its tools.",
			details: extension.permissions.map((permission) => `${permission.kind}: ${permission.value}`),
			confirm: "Remove MCP server",
			danger: true
		})) return;
		const endpoint = extension.source_uri.startsWith("opencoding://extensions/mcp-http/") ? `/v1/extensions/mcp-http/${encodeURIComponent(extension.id)}` : `/v1/extensions/${encodeURIComponent(extension.id)}`;
		await api(endpoint, {
			method: "DELETE",
			body: JSON.stringify({
				scope: scope(),
				confirmation: {
					confirmed: true,
					permissions_sha256: extension.permissions_sha256
				}
			})
		});
		selectedExtensionId = null;
		await loadExtensionCatalog();
		toast("MCP server removed · restart Opencoding to finish");
	}
	function localPathToFileUri(path) {
		const normalized = path.trim().replaceAll("\\", "/");
		if (/^[A-Za-z]:\//.test(normalized)) return new URL(`file:///${normalized}`).toString();
		if (!normalized.startsWith("/")) throw new Error("Marketplace source must be an absolute local path");
		return new URL(normalized, "file:///").toString();
	}
	async function managePluginMarketplaces() {
		if (!state.connected) {
			toast("Connect to Opencoding before managing Plugin Marketplaces.");
			return;
		}
		const values = await requestAction({
			eyebrow: "Community Plugins",
			title: "Manage Plugin Marketplaces",
			description: "Community sources are explicit local directories or marketplace.json files. Adding, refreshing, and removal all require a permission review.",
			confirm: "Continue",
			fields: [
				{
					name: "action",
					label: "Action",
					options: [
						["list", "List"],
						["add", "Add"],
						["upgrade", "Refresh"],
						["remove", "Remove"]
					]
				},
				{
					name: "name",
					label: "Marketplace name",
					placeholder: "local"
				},
				{
					name: "path",
					label: "Absolute local source path (Add only)",
					placeholder: "/opt/opencoding-plugins"
				}
			]
		});
		if (!values) return;
		if (values.action === "list") {
			const marketplaces = await api(`/v1/marketplaces?${catalogQuery()}`);
			await requestAction({
				eyebrow: "Community Plugins",
				title: "Configured Marketplaces",
				description: marketplaces.length ? "These local sources are indexed for this Team." : "No Plugin Marketplaces are configured.",
				details: marketplaces.map((marketplace) => `${marketplace.source.name} · revision ${marketplace.revision} · ${marketplace.source.source_uri}`),
				confirm: "Close"
			});
			return;
		}
		const name = values.name.trim();
		if (!name) throw new Error("Marketplace name is required");
		let preview;
		let source = null;
		if (values.action === "add") {
			source = {
				name,
				source_uri: localPathToFileUri(values.path),
				source_kind: "local"
			};
			preview = await api("/v1/marketplaces/preview", {
				method: "POST",
				body: JSON.stringify({
					scope: scope(),
					source
				})
			});
		} else preview = await api(`/v1/marketplaces/${encodeURIComponent(name)}/preview-upgrade?${catalogQuery()}`, { method: "POST" });
		if (!await requestAction({
			eyebrow: "Marketplace permission review",
			title: `${values.action === "remove" ? "Remove" : values.action === "upgrade" ? "Refresh" : "Add"} ${name}?`,
			description: values.action === "remove" ? "Installed Plugins must be removed first. This removes the source registration, not arbitrary files." : "Review the exact local source and manifest digest before continuing.",
			details: preview.descriptor.permissions.map((permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`),
			confirm: values.action === "remove" ? "Remove Marketplace" : values.action === "upgrade" ? "Refresh Marketplace" : "Add Marketplace",
			danger: values.action === "remove"
		})) return;
		if (values.action === "add" && source) await api("/v1/marketplaces", {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				source,
				confirmation: {
					confirmed: true,
					permissions_sha256: preview.permissions_sha256
				}
			})
		});
		else await api(`/v1/marketplaces/${encodeURIComponent(name)}`, {
			method: values.action === "remove" ? "DELETE" : "POST",
			body: JSON.stringify({
				scope: scope(),
				confirmation: {
					confirmed: true,
					permissions_sha256: preview.permissions_sha256
				}
			})
		});
		await loadExtensionCatalog();
		toast(`Marketplace ${values.action === "upgrade" ? "refreshed" : values.action === "add" ? "added" : "removed"}`);
	}
	function pluginSelector(extension) {
		return extension.id.startsWith("plugin:") ? extension.id.slice(7) : extension.id;
	}
	async function installOrUpdatePlugin(extension) {
		const selector = pluginSelector(extension);
		const separator = selector.lastIndexOf("@");
		if (separator <= 0 || separator === selector.length - 1) throw new Error("Plugin identity is missing its Marketplace");
		const plugin_name = selector.slice(0, separator);
		const marketplace_name = selector.slice(separator + 1);
		const preview = await api("/v1/plugins/preview", {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				marketplace_name,
				plugin_name
			})
		});
		if (!await requestAction({
			eyebrow: "Plugin permission review",
			title: `${extension.status === "installed" ? "Update" : "Install"} ${preview.descriptor.name}?`,
			description: "The reviewed Plugin package is frozen and installed atomically. Bundled MCP servers activate after restart.",
			details: [
				...preview.descriptor.permissions.map((permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`),
				`Version: ${preview.descriptor.version || "unspecified"}`,
				`Restart required: ${preview.requires_restart ? "yes" : "no"}`
			],
			confirm: extension.status === "installed" ? "Update Plugin" : "Install Plugin"
		})) return;
		await api("/v1/plugins/install", {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				marketplace_name,
				plugin_name,
				confirmation: {
					confirmed: true,
					permissions_sha256: preview.permissions_sha256
				}
			})
		});
		await loadExtensionCatalog();
		toast(preview.requires_restart ? "Plugin installed · restart Opencoding to activate bundled MCP servers" : "Plugin installed");
	}
	async function setPluginEnabled(extension, enabled) {
		const selector = pluginSelector(extension);
		const detail = await api(`/v1/plugins/${encodeURIComponent(selector)}?${catalogQuery()}`);
		if (detail.summary.installed_revision == null) throw new Error("Plugin is not installed");
		await api(`/v1/plugins/${encodeURIComponent(selector)}`, {
			method: "PUT",
			body: JSON.stringify({
				scope: scope(),
				enabled,
				expected_revision: detail.summary.installed_revision
			})
		});
		await loadExtensionCatalog();
		toast(`${enabled ? "Enabled" : "Disabled"} ${extension.name}`);
	}
	async function removePlugin(extension) {
		const selector = pluginSelector(extension);
		const detail = await api(`/v1/plugins/${encodeURIComponent(selector)}?${catalogQuery()}`);
		const digest = detail.summary.descriptor.permissions_sha256;
		if (!digest) throw new Error("Plugin installation has no removable permission revision");
		if (!await requestAction({
			eyebrow: "Community Plugin",
			title: `Remove ${extension.name}?`,
			description: "The frozen Plugin bundle and all of its Skill, Hook, MCP, App, and Agent contents will be removed together.",
			details: detail.summary.components.map((component) => `${component.kind}: ${component.name}`),
			confirm: "Remove Plugin",
			danger: true
		})) return;
		await api(`/v1/plugins/${encodeURIComponent(selector)}`, {
			method: "DELETE",
			body: JSON.stringify({
				scope: scope(),
				confirmation: {
					confirmed: true,
					permissions_sha256: digest
				}
			})
		});
		selectedExtensionId = null;
		await loadExtensionCatalog();
		toast("Plugin removed");
	}
	async function enrichPluginDetail(extension, target) {
		const detail = await api(`/v1/plugins/${encodeURIComponent(pluginSelector(extension))}?${catalogQuery()}`);
		if (selectedExtensionId !== extension.id) return;
		const heading = document.createElement("strong");
		heading.textContent = `Contents · ${detail.summary.components.length}`;
		const contents = document.createElement("div");
		contents.className = "extension-permissions";
		detail.summary.components.forEach((component) => {
			const row = document.createElement("article");
			row.className = "extension-permission";
			const kind = document.createElement("strong");
			kind.textContent = component.kind;
			const name = document.createElement("span");
			name.textContent = component.name;
			const description = document.createElement("small");
			description.textContent = component.description;
			row.append(kind, name, description);
			contents.append(row);
		});
		target.append(heading, contents);
	}
	function parseLines(value) {
		return value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
	}
	async function addSkill() {
		if (!state.connected) {
			toast("Connect to Opencoding before installing a Skill.");
			return;
		}
		const values = await requestAction({
			eyebrow: "Local instructions",
			title: "Add a Skill",
			description: "Opencoding reads and freezes one reviewed SKILL.md revision. Automatic matching is optional; $skill-id always invokes an enabled Skill.",
			confirm: "Preview permissions",
			fields: [
				{
					name: "id",
					label: "Skill ID",
					placeholder: "secure-review",
					required: true,
					maxlength: 64
				},
				{
					name: "name",
					label: "Display name",
					placeholder: "Secure review",
					required: true,
					maxlength: 80
				},
				{
					name: "summary",
					label: "Description",
					placeholder: "Review access-control-sensitive changes.",
					required: true,
					maxlength: 500
				},
				{
					name: "source",
					label: "SKILL.md file URI",
					placeholder: "file:///Users/me/.opencoding/skills/secure-review/SKILL.md",
					required: true
				},
				{
					name: "terms",
					label: "Automatic match terms (one per line)",
					multiline: true,
					placeholder: "access control review\nsecurity review"
				},
				{
					name: "dependencies",
					label: "Required MCP server IDs (one per line)",
					multiline: true,
					placeholder: "repository"
				},
				{
					name: "activation",
					label: "Activation",
					value: "automatic",
					options: [["automatic", "Automatic terms + $skill-id"], ["manual", "$skill-id only"]]
				}
			]
		});
		if (!values) return;
		const terms = parseLines(values.terms);
		if (values.activation === "automatic" && !terms.length) throw new Error("Automatic Skill activation requires at least one match term");
		const skill = {
			id: values.id.trim(),
			name: values.name.trim(),
			description: values.summary.trim(),
			source_uri: values.source.trim(),
			activation_terms: terms,
			mcp_dependencies: parseLines(values.dependencies),
			auto_match: values.activation === "automatic"
		};
		const preview = await api("/v1/extensions/skills/preview", {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				skill
			})
		});
		if (!await requestAction({
			eyebrow: "Permission review",
			title: `Install ${preview.descriptor.name}?`,
			description: "Confirm the exact reviewed file digest, activation scope and dependencies. The Skill becomes available immediately.",
			details: [
				...preview.descriptor.permissions.map((permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`),
				`Scope: Team ${scope().team_id}`,
				`Source: ${preview.descriptor.source_uri}`,
				`Restart required: ${preview.requires_restart ? "yes" : "no"}`
			],
			confirm: "Install Skill"
		})) return;
		await api("/v1/extensions/skills/install", {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				skill,
				confirmation: {
					confirmed: true,
					permissions_sha256: preview.permissions_sha256
				}
			})
		});
		await loadExtensionCatalog();
		toast(`Skill installed · invoke with $${skill.id}`);
	}
	async function setSkillEnabled(extension, enabled) {
		const skillId = extension.id.startsWith("skill:") ? extension.id.slice(6) : extension.id;
		const installation = await api(`/v1/extensions/skills/${encodeURIComponent(skillId)}?${catalogQuery()}`);
		await api(`/v1/extensions/skills/${encodeURIComponent(skillId)}`, {
			method: "PUT",
			body: JSON.stringify({
				scope: scope(),
				enabled,
				expected_revision: installation.revision
			})
		});
		await loadExtensionCatalog();
		toast(`Skill ${enabled ? "enabled" : "disabled"}`);
	}
	async function removeSkill(extension) {
		if (!extension.permissions_sha256) throw new Error("This Skill installation has no removable permission revision");
		if (!await requestAction({
			eyebrow: "Local instructions",
			title: `Remove ${extension.name}?`,
			description: "The reviewed instructions will stop matching new Turns immediately.",
			details: extension.permissions.map((permission) => `${permission.kind}: ${permission.value}`),
			confirm: "Remove Skill",
			danger: true
		})) return;
		const skillId = extension.id.startsWith("skill:") ? extension.id.slice(6) : extension.id;
		await api(`/v1/extensions/skills/${encodeURIComponent(skillId)}`, {
			method: "DELETE",
			body: JSON.stringify({
				scope: scope(),
				confirmation: {
					confirmed: true,
					permissions_sha256: extension.permissions_sha256
				}
			})
		});
		selectedExtensionId = null;
		await loadExtensionCatalog();
		toast("Skill removed");
	}
	async function addHook() {
		if (!state.connected) {
			toast("Connect to Opencoding before installing a Hook.");
			return;
		}
		const values = await requestAction({
			eyebrow: "Local automation",
			title: "Add a Tool Hook",
			description: "Hooks receive bounded structured JSON. Use an absolute executable path; secrets remain in environment variables.",
			confirm: "Preview permissions",
			fields: [
				{
					name: "id",
					label: "Hook ID",
					placeholder: "validate-edits",
					required: true,
					maxlength: 64
				},
				{
					name: "name",
					label: "Display name",
					placeholder: "Validate edits",
					required: true,
					maxlength: 80
				},
				{
					name: "event",
					label: "Event",
					value: "pre_tool_use",
					options: [["pre_tool_use", "Before Tool Use"], ["post_tool_use", "After Tool Use"]]
				},
				{
					name: "program",
					label: "Absolute executable path",
					placeholder: "/usr/local/bin/example-hook",
					required: true
				},
				{
					name: "args",
					label: "Arguments (one per line)",
					multiline: true,
					placeholder: "--format\njson"
				},
				{
					name: "environment",
					label: "Environment handles (NAME=VARIABLE)",
					multiline: true,
					placeholder: "API_TOKEN=EXAMPLE_API_TOKEN"
				},
				{
					name: "timeout",
					label: "Timeout (milliseconds)",
					type: "number",
					min: 100,
					max: 1e4,
					value: 3e3,
					required: true
				},
				{
					name: "modify",
					label: "May replace Tool arguments",
					value: "false",
					options: [["false", "No"], ["true", "Yes — before Tool Use only"]]
				}
			]
		});
		if (!values) return;
		if (values.event === "post_tool_use" && values.modify === "true") throw new Error("Only a Before Tool Use Hook may replace Tool arguments");
		const hook = {
			id: values.id.trim(),
			name: values.name.trim(),
			event: values.event,
			program: values.program.trim(),
			args: values.args.split(/\r?\n/).map((line) => line.trim()).filter(Boolean),
			environment_handles: parseEnvironmentHandles(values.environment),
			timeout_ms: Number(values.timeout),
			can_modify_input: values.modify === "true"
		};
		const preview = await api("/v1/extensions/hooks/preview", {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				hook
			})
		});
		if (!await requestAction({
			eyebrow: "Permission review",
			title: `Install ${preview.descriptor.name}?`,
			description: "Review every effective permission. The Hook becomes active immediately after installation.",
			details: [
				...preview.descriptor.permissions.map((permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`),
				`Scope: Team ${scope().team_id}`,
				`Source: ${preview.descriptor.source_uri}`,
				`Restart required: ${preview.requires_restart ? "yes" : "no"}`
			],
			confirm: "Install Hook"
		})) return;
		await api("/v1/extensions/hooks/install", {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				hook,
				confirmation: {
					confirmed: true,
					permissions_sha256: preview.permissions_sha256
				}
			})
		});
		await loadExtensionCatalog();
		toast("Hook installed");
	}
	async function removeHook(extension) {
		if (!extension.permissions_sha256) throw new Error("This Hook installation has no removable permission revision");
		if (!await requestAction({
			eyebrow: "Local automation",
			title: `Remove ${extension.name}?`,
			description: "The Hook will stop running for new Tool calls immediately.",
			details: extension.permissions.map((permission) => `${permission.kind}: ${permission.value}`),
			confirm: "Remove Hook",
			danger: true
		})) return;
		const hookId = extension.id.startsWith("hook:") ? extension.id.slice(5) : extension.id;
		await api(`/v1/extensions/hooks/${encodeURIComponent(hookId)}`, {
			method: "DELETE",
			body: JSON.stringify({
				scope: scope(),
				confirmation: {
					confirmed: true,
					permissions_sha256: extension.permissions_sha256
				}
			})
		});
		selectedExtensionId = null;
		await loadExtensionCatalog();
		toast("Hook removed");
	}
	function showExtensions({ replace = false, updateRoute = true } = {}) {
		closeDrawers(false);
		document.body.classList.add("team-mode");
		$("team-view").hidden = true;
		$("projects-view").hidden = true;
		$("artifacts-view").hidden = true;
		$("extensions-view").hidden = false;
		$("workspace-shell").hidden = true;
		$("open-team").classList.remove("active");
		$("open-team").removeAttribute("aria-current");
		document.body.classList.remove("mobile-sidebar-open");
		closeUserMenu();
		if (updateRoute) routePath(extensionsRoute(), replace);
		loadExtensionCatalog().catch((error) => toast(error.message));
		$("extensions-title").focus({ preventScroll: true });
	}
	return {
		addHook,
		addMcpServer,
		addSkill,
		loadExtensionCatalog,
		managePluginMarketplaces,
		renderExtensionList,
		showExtensions
	};
}
//#endregion
//#region src/pages/team-audit.ts
function renderTeamAudit$1(container, page) {
	container.replaceChildren();
	container.className = "team-queue";
	if (!page.events.length) {
		container.textContent = "No audit activity";
		container.classList.add("empty");
		return;
	}
	[...page.events].reverse().forEach((event) => {
		const row = document.createElement("article");
		row.className = "task";
		const title = document.createElement("div");
		title.className = "task-title";
		title.textContent = event.kind.replaceAll(".", " · ");
		const sequence = document.createElement("span");
		sequence.className = "task-status";
		sequence.textContent = `#${event.sequence}`;
		title.append(sequence);
		const meta = document.createElement("div");
		meta.className = "task-meta";
		meta.textContent = [
			new Date(event.timestamp).toLocaleString(),
			`actor ${event.actor_id}`,
			event.goal_id ? `goal ${event.goal_id}` : null,
			event.task_id ? `task ${event.task_id}` : null,
			event.session_id ? `session ${event.session_id}` : null,
			event.turn_id ? `turn ${event.turn_id}` : null,
			event.item_id ? `item ${event.item_id}` : null,
			event.request_id ? `request ${event.request_id}` : null
		].filter(Boolean).join(" · ");
		row.append(title, meta);
		container.append(row);
	});
}
function renderTeamGovernance$1(container, governance) {
	container.replaceChildren();
	container.className = "governance-summary";
	const rows = [
		["Source", governance.source.replaceAll("_", " ")],
		["Audit content", governance.audit_content_enabled ? "Encrypted under the signed Team policy" : "Metadata only"],
		["Retention", governance.audit_retention_days == null ? "No content-retention policy" : `${governance.audit_retention_days} days · Legal Hold takes precedence`],
		["Data residency", governance.data_residency_region || "Local Team storage"]
	];
	if (governance.configuration_sequence != null) rows.push(["Signed revision", `configuration ${governance.configuration_sequence} · policy ${governance.policy_sequence}`]);
	if (governance.expires_at) rows.push(["Valid until", new Date(governance.expires_at).toLocaleString()]);
	rows.forEach(([label, value]) => {
		const row = document.createElement("article");
		const heading = document.createElement("strong");
		heading.textContent = label;
		const detail = document.createElement("span");
		detail.textContent = value;
		row.append(heading, detail);
		container.append(row);
	});
}
//#endregion
//#region src/pages/team-work.ts
function createTeamWorkPage(context) {
	const { lookup: $, api, state, scope, catalogQuery, requestAction, toast, addActivity, selectSession, refreshSessions, openDrawer, announce, showArtifacts, showWorkspace, composerTextDraftKey, resizePrompt } = context;
	function teamQuery(extra = {}) {
		const s = scope();
		return new URLSearchParams({
			organization_id: s.organization_id,
			actor_id: s.actor_id,
			...extra
		});
	}
	function auditQueryParameters() {
		const query = teamQuery({ limit: "100" });
		[
			["filter_actor_id", "audit-filter-actor"],
			["kind", "audit-filter-action"],
			["session_id", "audit-filter-session"]
		].forEach(([parameter, id]) => {
			const value = $(id).value.trim();
			if (value) query.set(parameter, value);
		});
		[["since", "audit-filter-since"], ["until", "audit-filter-until"]].forEach(([parameter, id]) => {
			const value = $(id).value;
			if (value) query.set(parameter, new Date(value).toISOString());
		});
		return query;
	}
	async function exportTeamAudit() {
		if (!state.connected) throw new Error("Connect before exporting Team audit");
		const team = encodeURIComponent(scope().team_id);
		const response = await fetch(`/v1/teams/${team}/audit/export?${auditQueryParameters()}`, {
			credentials: "same-origin",
			headers: { Accept: "text/csv" }
		});
		if (!response.ok) throw new Error(`Audit export failed (${response.status})`);
		const blob = await response.blob();
		const href = URL.createObjectURL(blob);
		try {
			const link = document.createElement("a");
			link.href = href;
			link.download = `opencoding-team-audit-${(/* @__PURE__ */ new Date()).toISOString().slice(0, 10)}.csv`;
			link.click();
		} finally {
			URL.revokeObjectURL(href);
		}
		toast("Content-free audit CSV exported");
	}
	async function refreshTeam() {
		const generation = state.generation;
		try {
			const team = encodeURIComponent(scope().team_id);
			const query = teamQuery();
			const [dashboard, tasks, goals, capacityResult, budgetsResult, ownershipResult, durableTasks, backgroundTerminals, agentRuns, approvals, outcomes, audit, governance] = await Promise.all([
				api(`/v1/teams/${team}/dashboard?${query}`),
				api(`/v1/teams/${team}/tasks?${query}`),
				api(`/v1/teams/${team}/goals?${query}`),
				api(`/v1/teams/${team}/capacity?${query}`).catch(() => null),
				api(`/v1/teams/${team}/budgets?${query}`).catch(() => []),
				api(`/v1/teams/${team}/ownership?${query}`).catch(() => []),
				api(`/v1/durable-task-summaries?${catalogQuery()}`).catch(() => []),
				api(`/v1/background-terminals?${catalogQuery()}`).catch(() => []),
				api(`/v1/agents?${catalogQuery()}`).catch(() => []),
				api(`/v1/teams/${team}/approvals?${query}`).catch(() => []),
				api(`/v1/teams/${team}/outcomes?${query}`).catch(() => []),
				api(`/v1/teams/${team}/audit?${auditQueryParameters()}`).catch(() => ({
					events: [],
					next_cursor: null
				})),
				api(`/v1/teams/${team}/governance?${query}`).catch(() => ({
					source: "unavailable",
					configuration_sequence: null,
					policy_sequence: null,
					issued_at: null,
					expires_at: null,
					audit_content_enabled: false,
					audit_retention_days: null,
					data_residency_region: null,
					audit_content_categories: []
				}))
			]);
			if (!state.connected || generation !== state.generation) return;
			const metrics = $("team-metrics");
			metrics.replaceChildren();
			[
				["Outcomes", dashboard.verified_outcomes],
				["Ready", dashboard.ready_tasks],
				["In progress", dashboard.in_progress_tasks],
				["Blocked", dashboard.blocked_tasks]
			].forEach(([label, value]) => {
				const card = document.createElement("div");
				card.className = "metric";
				const number = document.createElement("strong");
				number.textContent = String(value);
				const name = document.createElement("span");
				name.textContent = String(label);
				card.append(number, name);
				metrics.append(card);
			});
			if (capacityResult) [
				["WIP", `${dashboard.in_progress_tasks}/${capacityResult.wip_limit}`],
				["Agents", capacityResult.agent_concurrency],
				["Human h", capacityResult.human_available_hours]
			].forEach(([label, value]) => {
				const card = document.createElement("div");
				card.className = "metric";
				const number = document.createElement("strong");
				number.textContent = String(value);
				const name = document.createElement("span");
				name.textContent = String(label);
				card.append(number, name);
				metrics.append(card);
			});
			const activeBudget = budgetsResult.find((budget) => new Date(budget.period_start) <= /* @__PURE__ */ new Date() && new Date(budget.period_end) > /* @__PURE__ */ new Date());
			if (activeBudget) {
				const available = Math.max(0, activeBudget.model_limit_micros - activeBudget.model_consumed_micros - activeBudget.model_reserved_micros);
				const card = document.createElement("div");
				card.className = "metric";
				const number = document.createElement("strong");
				number.textContent = `$${(available / 1e6).toFixed(2)}`;
				const name = document.createElement("span");
				name.textContent = "Model available";
				card.append(number, name);
				metrics.append(card);
			}
			const goalRuns = (await Promise.all(goals.map((goal) => api(`/v1/teams/${team}/goals/${encodeURIComponent(goal.id)}/runs?${query}`).catch(() => [])))).flat();
			if (!state.connected || generation !== state.generation) return;
			renderGoals(goals, goalRuns);
			renderOwnership(ownershipResult);
			renderTasks(tasks);
			renderDurableTasks(durableTasks);
			renderBackgroundTerminals(backgroundTerminals);
			renderAgentRuns(agentRuns);
			renderCapacity(capacityResult, dashboard.in_progress_tasks);
			renderBudgets(budgetsResult);
			renderTeamApprovals(approvals);
			renderTeamOutcomes(outcomes);
			renderTeamAudit(audit);
			renderTeamGovernance(governance);
		} catch (error) {
			if (state.connected && generation === state.generation) addActivity("team.error", { error: error.message });
		}
	}
	function renderCapacity(capacity, inProgress) {
		const container = $("capacity-summary");
		container.replaceChildren();
		if (!capacity) {
			const empty = document.createElement("p");
			empty.className = "empty";
			empty.textContent = "No capacity limits configured.";
			container.append(empty);
			return;
		}
		[
			["Work in progress", `${inProgress}/${capacity.wip_limit}`],
			["Agent concurrency", capacity.agent_concurrency],
			["Human hours", capacity.human_available_hours]
		].forEach(([label, value]) => {
			const card = document.createElement("div");
			card.className = "metric";
			const number = document.createElement("strong");
			number.textContent = String(value);
			const name = document.createElement("span");
			name.textContent = String(label);
			card.append(number, name);
			container.append(card);
		});
	}
	function renderBudgets(budgets) {
		const container = $("team-budget-list");
		container.replaceChildren();
		container.className = "team-queue";
		if (!budgets.length) {
			container.textContent = "No budgets configured";
			container.classList.add("empty");
			return;
		}
		budgets.forEach((budget) => {
			const row = document.createElement("article");
			row.className = "task";
			const title = document.createElement("div");
			title.className = "task-title";
			title.textContent = `${budget.hard_limit ? "Hard" : "Advisory"} budget`;
			const meta = document.createElement("div");
			meta.className = "task-meta";
			meta.textContent = [
				`${new Date(budget.period_start).toLocaleDateString()}–${new Date(budget.period_end).toLocaleDateString()}`,
				`model $${(budget.model_consumed_micros / 1e6).toFixed(2)} / $${(budget.model_limit_micros / 1e6).toFixed(2)}`,
				`runner $${(budget.runner_consumed_micros / 1e6).toFixed(2)} / $${(budget.runner_limit_micros / 1e6).toFixed(2)}`
			].join(" · ");
			row.append(title, meta);
			container.append(row);
		});
	}
	function renderTeamApprovals(approvals) {
		const container = $("team-approval-list");
		container.replaceChildren();
		container.className = "team-queue";
		if (!approvals.length) {
			container.textContent = "No approval decisions";
			container.classList.add("empty");
			return;
		}
		approvals.forEach((approval) => {
			const row = document.createElement("article");
			row.className = "task";
			const title = document.createElement("div");
			title.className = "task-title";
			title.textContent = approval.summary;
			const status = document.createElement("span");
			status.className = "task-status";
			status.textContent = approval.status;
			title.append(status);
			const meta = document.createElement("div");
			meta.className = "task-meta";
			meta.textContent = `${approval.risk} risk · ${approval.impact_scope} · ${new Date(approval.requested_at).toLocaleString()}`;
			const actions = document.createElement("div");
			actions.className = "task-buttons";
			const session = state.sessions.find((candidate) => candidate.id === approval.session_id);
			if (session) {
				const open = document.createElement("button");
				open.type = "button";
				open.textContent = "Open Session";
				open.addEventListener("click", () => selectSession(session));
				actions.append(open);
			}
			if (approval.status === "pending") [["Approve once", true], ["Reject", false]].forEach(([label, approved]) => {
				const button = document.createElement("button");
				button.type = "button";
				button.textContent = label;
				button.classList.toggle("danger", !approved);
				button.addEventListener("click", async () => {
					await api(`/v1/approvals/${encodeURIComponent(approval.id)}`, {
						method: "POST",
						body: JSON.stringify({
							scope: scope(),
							approved,
							approval_scope: "once"
						})
					});
					await refreshTeam();
					toast(approved ? "Approval granted once" : "Approval rejected");
				});
				actions.append(button);
			});
			row.append(title, meta, actions);
			container.append(row);
		});
	}
	function renderTeamOutcomes(outcomes) {
		const renderInto = (container, visible) => {
			container.replaceChildren();
			container.className = "team-queue";
			if (!visible.length) {
				container.textContent = "No verified outcomes";
				container.classList.add("empty");
				return;
			}
			visible.forEach((outcome) => {
				const row = document.createElement("article");
				row.className = "task";
				const title = document.createElement("div");
				title.className = "task-title";
				title.textContent = `Verified outcome · ${outcome.task_id}`;
				const meta = document.createElement("div");
				meta.className = "task-meta";
				meta.textContent = `${outcome.evidence.length} evidence item${outcome.evidence.length === 1 ? "" : "s"} · ${new Date(outcome.completed_at).toLocaleString()}`;
				const evidence = document.createElement("div");
				evidence.className = "task-meta";
				evidence.textContent = outcome.evidence.map((item) => `${item.kind}: ${item.result}`).join(" · ");
				row.append(title, meta, evidence);
				if (outcome.pull_request_url) {
					const link = document.createElement("a");
					link.href = outcome.pull_request_url;
					link.target = "_blank";
					link.rel = "noopener noreferrer";
					link.textContent = "Open pull request";
					row.append(link);
				}
				container.append(row);
			});
		};
		renderInto($("team-outcome-list"), outcomes);
		renderInto($("outcome-summary"), outcomes.slice(0, 5));
	}
	function renderTeamAudit(page) {
		renderTeamAudit$1($("team-audit-list"), page);
	}
	function renderTeamGovernance(governance) {
		renderTeamGovernance$1($("team-governance-summary"), governance);
	}
	function durableSession(task) {
		return state.sessions.find((session) => session.id === task.session_id) || null;
	}
	function renderDurableTasks(tasks) {
		const container = $("durable-task-list");
		container.replaceChildren();
		container.className = "team-queue";
		const visible = [...tasks].sort((left, right) => String(right.updated_at).localeCompare(String(left.updated_at))).slice(0, 12);
		if (!visible.length) {
			container.textContent = "No background tasks";
			container.classList.add("empty");
			return;
		}
		visible.forEach((task) => {
			const row = document.createElement("article");
			row.className = "task";
			const heading = document.createElement("div");
			heading.className = "task-title";
			const title = document.createElement("span");
			title.textContent = durableSession(task)?.title || task.kind;
			const status = document.createElement("span");
			status.className = "task-status";
			status.textContent = task.status;
			heading.append(title, status);
			const meta = document.createElement("div");
			meta.className = "task-meta";
			meta.textContent = `attempt ${task.attempt}/${task.max_attempts} · model $${(task.consumed_cost_micros / 1e6).toFixed(4)} · runner $${(task.consumed_runner_cost_micros / 1e6).toFixed(4)} · ${new Date(task.updated_at).toLocaleString()}`;
			const actions = document.createElement("div");
			actions.className = "task-buttons";
			const sourceSession = durableSession(task);
			if (sourceSession) {
				const open = document.createElement("button");
				open.type = "button";
				open.textContent = "Open in Chat";
				open.addEventListener("click", () => {
					selectSession(sourceSession).catch((error) => toast(error.message));
				});
				actions.append(open);
			}
			if ([
				"queued",
				"leased",
				"running"
			].includes(task.status)) {
				const pause = document.createElement("button");
				pause.type = "button";
				pause.textContent = "Pause";
				pause.addEventListener("click", () => controlDurableTask(task, "pause"));
				actions.append(pause);
			}
			if (task.status === "paused") {
				const resume = document.createElement("button");
				resume.type = "button";
				resume.textContent = "Resume";
				resume.addEventListener("click", () => controlDurableTask(task, "resume"));
				actions.append(resume);
			}
			if (![
				"succeeded",
				"failed",
				"cancelled"
			].includes(task.status)) {
				const cancel = document.createElement("button");
				cancel.type = "button";
				cancel.textContent = "Cancel";
				cancel.addEventListener("click", () => controlDurableTask(task, "cancel"));
				actions.append(cancel);
			}
			row.append(heading, meta, actions);
			container.append(row);
		});
	}
	async function controlDurableTask(task, action) {
		if (action === "cancel") {
			if (!await requestAction({
				eyebrow: "Background task",
				title: "Cancel this task?",
				description: "The current lease is revoked and the task will not retry.",
				details: [
					`Task: ${task.id}`,
					`Status: ${task.status}`,
					"Session history and audit evidence remain available."
				],
				confirm: "Cancel task",
				danger: true
			})) return;
		}
		await api(`/v1/durable-tasks/${encodeURIComponent(task.id)}/${action}`, {
			method: "POST",
			body: JSON.stringify(scope())
		});
		await refreshTeam();
		toast(`Background task ${action === "resume" ? "resumed" : `${action}d`}`);
	}
	function backgroundTerminalSession(terminal) {
		return state.sessions.find((session) => session.id === terminal.session_id) || null;
	}
	async function readBackgroundTerminalOutput(terminal) {
		const decoder = new TextDecoder();
		let text = "";
		let offset = 0;
		for (let page = 0; page < 16; page += 1) {
			const query = new URLSearchParams(catalogQuery());
			query.set("offset", String(offset));
			query.set("limit", "65536");
			const output = await api(`/v1/background-terminals/${encodeURIComponent(terminal.id)}/output?${query}`);
			const binary = atob(output.content_base64);
			const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
			text += decoder.decode(bytes, { stream: !output.eof });
			if (output.eof || output.next_offset === offset) break;
			offset = output.next_offset;
		}
		text += decoder.decode();
		return text;
	}
	async function writeBackgroundTerminal(terminal) {
		const values = await requestAction({
			eyebrow: "Background terminal",
			title: "Send terminal input",
			description: "A newline is appended so line-oriented programs receive the input immediately.",
			confirm: "Send input",
			fields: [{
				name: "content",
				label: "Input",
				multiline: true,
				required: true,
				maxlength: 65e3
			}]
		});
		if (!values) return;
		const content = new TextEncoder().encode(`${values.content}\n`);
		let binary = "";
		content.forEach((byte) => {
			binary += String.fromCharCode(byte);
		});
		await api(`/v1/background-terminals/${encodeURIComponent(terminal.id)}/input`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				content_base64: btoa(binary)
			})
		});
		toast("Terminal input sent");
	}
	async function resizeBackgroundTerminal(terminal) {
		const values = await requestAction({
			eyebrow: "Background terminal",
			title: "Resize terminal",
			description: "Resize the PTY without restarting the process.",
			confirm: "Resize",
			fields: [{
				name: "rows",
				label: "Rows",
				type: "number",
				min: 2,
				max: 500,
				value: terminal.rows,
				required: true
			}, {
				name: "cols",
				label: "Columns",
				type: "number",
				min: 2,
				max: 500,
				value: terminal.cols,
				required: true
			}]
		});
		if (!values) return;
		await api(`/v1/background-terminals/${encodeURIComponent(terminal.id)}/resize`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				rows: Number(values.rows),
				cols: Number(values.cols)
			})
		});
		await refreshTeam();
		toast("Terminal resized");
	}
	async function stopBackgroundTerminal(terminal) {
		if (!await requestAction({
			eyebrow: "Background terminal",
			title: "Stop this terminal?",
			description: "The daemon stops the process and preserves retained output as an Artifact.",
			details: [`Terminal: ${terminal.id}`, `Program: ${terminal.program}`],
			confirm: "Stop terminal",
			danger: true
		})) return;
		const latest = (await api(`/v1/background-terminals?${catalogQuery()}`)).find((candidate) => candidate.id === terminal.id);
		if (!latest) throw new Error("The background terminal is no longer available");
		await api(`/v1/background-terminals/${encodeURIComponent(terminal.id)}/stop`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				expected_revision: latest.revision
			})
		});
		toast("Terminal stop requested");
	}
	function renderBackgroundTerminals(terminals) {
		const container = $("background-terminal-list");
		container.replaceChildren();
		container.className = "team-queue";
		const visible = [...terminals].sort((left, right) => String(right.updated_at).localeCompare(String(left.updated_at))).slice(0, 12);
		if (!visible.length) {
			container.textContent = "No background terminals";
			container.classList.add("empty");
			return;
		}
		visible.forEach((terminal) => {
			const row = document.createElement("article");
			row.className = "task background-terminal";
			row.dataset.terminalId = terminal.id;
			const heading = document.createElement("div");
			heading.className = "task-title";
			const title = document.createElement("span");
			title.textContent = terminal.program;
			const status = document.createElement("span");
			status.className = "task-status";
			status.textContent = terminal.status;
			heading.append(title, status);
			const meta = document.createElement("div");
			meta.className = "task-meta";
			meta.textContent = [
				`${terminal.cols}×${terminal.rows}`,
				`${terminal.argument_count} argument${terminal.argument_count === 1 ? "" : "s"}`,
				`${terminal.output_byte_length.toLocaleString()} output bytes`,
				terminal.exit_code == null ? null : `exit ${terminal.exit_code}`,
				terminal.output_truncated ? "retained tail only" : null
			].filter(Boolean).join(" · ");
			const actions = document.createElement("div");
			actions.className = "task-buttons";
			const sourceSession = backgroundTerminalSession(terminal);
			if (sourceSession) {
				const open = document.createElement("button");
				open.type = "button";
				open.textContent = "Open Session";
				open.addEventListener("click", () => {
					selectSession(sourceSession).catch((error) => toast(error.message));
				});
				actions.append(open);
			}
			const output = document.createElement("button");
			output.type = "button";
			output.textContent = "Output";
			const outputPanel = document.createElement("pre");
			outputPanel.className = "terminal-output";
			outputPanel.hidden = true;
			output.addEventListener("click", () => {
				if (!outputPanel.hidden) {
					outputPanel.hidden = true;
					output.textContent = "Output";
					return;
				}
				output.disabled = true;
				readBackgroundTerminalOutput(terminal).then((content) => {
					outputPanel.textContent = content || "Terminal has not produced output.";
					outputPanel.hidden = false;
					output.textContent = "Hide output";
				}).catch((error) => toast(error.message)).finally(() => {
					output.disabled = false;
				});
			});
			actions.append(output);
			if (terminal.status === "running") {
				const input = document.createElement("button");
				input.type = "button";
				input.textContent = "Input";
				input.addEventListener("click", () => {
					writeBackgroundTerminal(terminal).catch((error) => toast(error.message));
				});
				const resize = document.createElement("button");
				resize.type = "button";
				resize.textContent = "Resize";
				resize.addEventListener("click", () => {
					resizeBackgroundTerminal(terminal).catch((error) => toast(error.message));
				});
				const stop = document.createElement("button");
				stop.type = "button";
				stop.textContent = "Stop";
				stop.className = "danger";
				stop.addEventListener("click", () => {
					stopBackgroundTerminal(terminal).catch((error) => toast(error.message));
				});
				actions.append(input, resize, stop);
			}
			if (terminal.artifact_id) {
				const artifact = document.createElement("button");
				artifact.type = "button";
				artifact.textContent = "Open Artifact";
				artifact.addEventListener("click", () => showArtifacts(terminal.artifact_id));
				actions.append(artifact);
			}
			row.append(heading, meta, actions, outputPanel);
			container.append(row);
		});
	}
	function agentDepth(agent, byId) {
		let depth = 0;
		let parent = agent.parent_id;
		const seen = /* @__PURE__ */ new Set([agent.id]);
		while (parent && byId.has(parent) && !seen.has(parent) && depth < 8) {
			seen.add(parent);
			depth += 1;
			parent = byId.get(parent)?.parent_id || null;
		}
		return depth;
	}
	function renderAgentRuns(agents) {
		const container = $("agent-run-list");
		container.replaceChildren();
		container.className = "team-queue";
		if (!agents.length) {
			container.textContent = "No agent runs";
			container.classList.add("empty");
			return;
		}
		const byId = new Map(agents.map((agent) => [agent.id, agent]));
		agents.sort((left, right) => String(left.created_at).localeCompare(String(right.created_at))).forEach((agent) => {
			const row = document.createElement("article");
			row.className = "task agent-run";
			row.style.setProperty("--agent-indent", `${agentDepth(agent, byId) * 14}px`);
			const heading = document.createElement("div");
			heading.className = "task-title";
			const title = document.createElement("span");
			title.textContent = agent.parent_id ? `Child agent · ${agent.id}` : `Agent · ${agent.id}`;
			const status = document.createElement("span");
			status.className = "task-status";
			status.textContent = agent.cancel_requested ? `${agent.status} · stopping` : agent.status;
			heading.append(title, status);
			const meta = document.createElement("div");
			meta.className = "task-meta";
			meta.textContent = `${agent.model || "configured model"} · attempt ${agent.attempt} · $${((agent.consumed_cost_micros + agent.consumed_runner_cost_micros) / 1e6).toFixed(4)}`;
			const actions = document.createElement("div");
			actions.className = "task-buttons";
			const details = document.createElement("button");
			details.type = "button";
			details.textContent = "Details";
			details.addEventListener("click", () => {
				showAgentDetails(agent).catch((error) => toast(error.message));
			});
			actions.append(details);
			if ([
				"succeeded",
				"failed",
				"cancelled"
			].includes(agent.status)) {
				const result = document.createElement("button");
				result.type = "button";
				result.textContent = "Result";
				result.addEventListener("click", () => {
					showAgentResult(agent).catch((error) => toast(error.message));
				});
				actions.append(result);
			}
			const sourceSession = state.sessions.find((session) => session.id === agent.session_id);
			if (sourceSession) {
				const open = document.createElement("button");
				open.type = "button";
				open.textContent = "Open in Chat";
				open.addEventListener("click", () => {
					selectSession(sourceSession).catch((error) => toast(error.message));
				});
				actions.append(open);
			}
			if (!agent.cancel_requested && !["failed", "cancelled"].includes(agent.status)) {
				const followUp = document.createElement("button");
				followUp.type = "button";
				followUp.textContent = "Follow-up";
				followUp.addEventListener("click", () => {
					followUpAgent(agent).catch((error) => toast(error.message));
				});
				actions.append(followUp);
			}
			if (![
				"succeeded",
				"failed",
				"cancelled"
			].includes(agent.status)) {
				const wait = document.createElement("button");
				wait.type = "button";
				wait.textContent = "Wait";
				wait.addEventListener("click", () => {
					waitForAgent(agent).catch((error) => toast(error.message));
				});
				actions.append(wait);
				const interrupt = document.createElement("button");
				interrupt.type = "button";
				interrupt.textContent = "Interrupt";
				interrupt.addEventListener("click", () => {
					interruptAgent(agent).catch((error) => toast(error.message));
				});
				actions.append(interrupt);
				const close = document.createElement("button");
				close.type = "button";
				close.textContent = "Close";
				close.addEventListener("click", () => {
					closeAgent(agent).catch((error) => toast(error.message));
				});
				actions.append(close);
			}
			row.append(heading, meta, actions);
			container.append(row);
		});
	}
	async function showAgentDetails(agent) {
		await requestAction({
			eyebrow: agent.parent_id ? "Child Agent" : "Agent",
			title: agent.id,
			description: `${agent.status}${agent.cancel_requested ? " · close requested" : ""}`,
			details: [
				`Parent: ${agent.parent_id || "root"}`,
				`Session: ${agent.session_id || "none"}`,
				`Goal: ${agent.goal_id || "none"}`,
				`Team task: ${agent.team_task_id || "none"}`,
				`Model: ${agent.model || "configured model"}`,
				`Attempt: ${agent.attempt}`,
				`Cost: model $${(agent.consumed_cost_micros / 1e6).toFixed(4)} · runner $${(agent.consumed_runner_cost_micros / 1e6).toFixed(4)}`,
				`Updated: ${new Date(agent.updated_at).toLocaleString()}`
			],
			confirm: "Done"
		});
	}
	async function showAgentResult(agent) {
		const result = await api(`/v1/agent-results/${encodeURIComponent(agent.id)}?${catalogQuery()}`);
		await requestAction({
			eyebrow: agent.parent_id ? "Child Agent result" : "Agent result",
			title: result.agent_id,
			description: result.summary || "No final assistant message was recorded.",
			details: [
				`Status: ${result.status}`,
				`Session: ${result.session_id}`,
				`Final message: ${result.final_message_id || "none"}`,
				`Content: ${result.summary_byte_length} bytes${result.truncated ? " · showing first 16 KiB" : ""}`,
				`Artifacts: ${result.artifact_count}`,
				`Cost: model $${(result.consumed_cost_micros / 1e6).toFixed(4)} · runner $${(result.consumed_runner_cost_micros / 1e6).toFixed(4)}`
			],
			confirm: "Done"
		});
	}
	async function waitForAgent(agent) {
		toast(`Waiting up to 30 seconds for ${agent.id}`);
		const result = await api(`/v1/agents/${encodeURIComponent(agent.id)}/wait`, {
			method: "POST",
			body: JSON.stringify({
				scope: {
					...scope(),
					goal_id: agent.goal_id,
					task_id: agent.team_task_id
				},
				timeout_seconds: 30
			})
		});
		await refreshTeam();
		const terminal = [
			"succeeded",
			"failed",
			"cancelled"
		].includes(result.status);
		toast(terminal ? `Agent finished · ${result.status}` : `Agent is still ${result.status}`);
	}
	async function closeAgent(agent) {
		if (!await requestAction({
			eyebrow: "Agent control",
			title: "Close this Agent?",
			description: "Requests a safe shutdown. An active worker keeps its lease until it records the final cancellation state.",
			details: [
				`Agent: ${agent.id}`,
				`Status: ${agent.status}`,
				"Session history and audit evidence remain available."
			],
			confirm: "Close Agent",
			danger: true
		})) return;
		const result = await api(`/v1/agents/${encodeURIComponent(agent.id)}/close`, {
			method: "POST",
			body: JSON.stringify({
				...scope(),
				goal_id: agent.goal_id,
				task_id: agent.team_task_id
			})
		});
		await refreshTeam();
		toast([
			"succeeded",
			"failed",
			"cancelled"
		].includes(result.status) ? "Agent closed" : "Agent close requested");
	}
	async function followUpAgent(agent) {
		const values = await requestAction({
			eyebrow: "Agent follow-up",
			title: "Send bounded follow-up work",
			description: "Creates a child Agent run in the same Session. The parent hierarchy, Team Task and Goal scope are preserved.",
			confirm: "Queue follow-up",
			fields: [
				{
					name: "content",
					label: "Message",
					multiline: true,
					maxlength: 8e3,
					placeholder: "Verify the edge case and report the exact test evidence.",
					required: true
				},
				{
					name: "attempts",
					label: "Maximum attempts",
					type: "number",
					min: 1,
					value: 1,
					required: true
				},
				{
					name: "modelBudget",
					label: "Delegated model budget (USD)",
					type: "number",
					min: 0,
					value: 1,
					required: true
				}
			]
		});
		if (!values) return;
		await api(`/v1/agents/${encodeURIComponent(agent.id)}/follow-up`, {
			method: "POST",
			body: JSON.stringify({
				scope: {
					...scope(),
					goal_id: agent.goal_id,
					task_id: agent.team_task_id
				},
				content: values.content.trim(),
				idempotency_key: `web-follow-up-${crypto.randomUUID()}`,
				max_attempts: Number(values.attempts),
				max_runtime_seconds: 3600,
				max_cost_micros: Math.round(Number(values.modelBudget) * 1e6)
			})
		});
		await refreshTeam();
		toast("Agent follow-up queued");
	}
	async function interruptAgent(agent) {
		if (!await requestAction({
			eyebrow: "Agent control",
			title: "Interrupt this Agent?",
			description: "The active lease is revoked and no retry will start. Session history and audit evidence remain available.",
			details: [`Agent: ${agent.id}`, `Status: ${agent.status}`],
			confirm: "Interrupt Agent",
			danger: true
		})) return;
		await api(`/v1/durable-tasks/${encodeURIComponent(agent.id)}/cancel`, {
			method: "POST",
			body: JSON.stringify({
				...scope(),
				goal_id: agent.goal_id,
				task_id: agent.team_task_id
			})
		});
		await refreshTeam();
		toast("Agent interrupted");
	}
	async function createBackgroundTask() {
		if (!state.sessions.length) {
			toast("Create a Session before starting a background Agent task.");
			showWorkspace();
			return;
		}
		const values = await requestAction({
			eyebrow: "Background Agent",
			title: "Run a task in the background",
			description: "The task continues in the daemon after this page closes. Its log stays in the source Session, not the Team overview.",
			confirm: "Queue background task",
			fields: [
				{
					name: "session",
					label: "Source Session",
					options: state.sessions.slice(0, 50).map((session) => [session.id, session.title])
				},
				{
					name: "content",
					label: "Task",
					multiline: true,
					maxlength: 8e3,
					placeholder: "Run the full test suite and fix the highest-impact failure.",
					required: true
				},
				{
					name: "attempts",
					label: "Maximum attempts",
					type: "number",
					min: 1,
					value: 3,
					required: true
				},
				{
					name: "modelBudget",
					label: "Model budget (USD)",
					type: "number",
					min: 0,
					value: 5,
					required: true
				}
			]
		});
		if (!values) return;
		const selected = state.sessions.find((session) => session.id === values.session);
		if (!selected) {
			toast("The selected Session is no longer available.");
			return;
		}
		const taskScope = {
			...scope(),
			goal_id: selected.scope?.goal_id || null,
			task_id: selected.scope?.task_id || null
		};
		await api("/v1/durable-tasks", {
			method: "POST",
			body: JSON.stringify({
				scope: taskScope,
				kind: "agent.turn",
				payload: {
					session_id: selected.id,
					content: values.content.trim()
				},
				idempotency_key: `web-${crypto.randomUUID()}`,
				max_attempts: Number(values.attempts),
				max_runtime_seconds: 3600,
				max_cost_micros: Math.round(Number(values.modelBudget) * 1e6),
				max_runner_cost_micros: 0
			})
		});
		await refreshTeam();
		toast("Background Agent task queued");
	}
	async function createBackgroundTerminal() {
		if (!state.sessions.length) {
			toast("Create a Session before starting a background terminal.");
			showWorkspace();
			return;
		}
		const values = await requestAction({
			eyebrow: "Background terminal",
			title: "Start a terminal process",
			description: "The process runs in the local daemon and continues after this page closes. Output is retained separately and becomes an Artifact when the process exits.",
			confirm: "Preview permissions",
			fields: [
				{
					name: "session",
					label: "Source Session",
					options: state.sessions.slice(0, 50).map((session) => [session.id, session.title])
				},
				{
					name: "program",
					label: "Absolute program path",
					value: "/bin/sh",
					placeholder: "/bin/sh",
					maxlength: 4096,
					required: true
				},
				{
					name: "arguments",
					label: "Arguments (one per line)",
					multiline: true,
					maxlength: 32e3,
					placeholder: "-c\nprintf 'hello from Opencoding\\n'"
				},
				{
					name: "environment",
					label: "Environment handles (NAME=HANDLE, one per line)",
					multiline: true,
					maxlength: 8e3,
					placeholder: "API_TOKEN=OPENROUTER_API_KEY"
				},
				{
					name: "rows",
					label: "Rows",
					type: "number",
					min: 2,
					max: 500,
					value: 24,
					required: true
				},
				{
					name: "cols",
					label: "Columns",
					type: "number",
					min: 2,
					max: 500,
					value: 80,
					required: true
				},
				{
					name: "runtime",
					label: "Maximum runtime (seconds)",
					type: "number",
					min: 1,
					max: 86400,
					value: 3600,
					required: true
				}
			]
		});
		if (!values) return;
		const session = state.sessions.find((candidate) => candidate.id === values.session);
		if (!session) throw new Error("The selected Session is no longer available");
		const environmentHandles = {};
		for (const entry of values.environment.split(/\r?\n/).map((line) => line.trim()).filter(Boolean)) {
			const separator = entry.indexOf("=");
			if (separator <= 0 || separator === entry.length - 1) throw new Error("Environment handles must use NAME=HANDLE");
			const name = entry.slice(0, separator);
			const handle = entry.slice(separator + 1);
			if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(handle)) throw new Error("Environment names and handles must use shell variable names");
			if (environmentHandles[name]) throw new Error(`Duplicate environment name ${name}`);
			environmentHandles[name] = handle;
		}
		const terminal = {
			session_id: session.id,
			program: values.program.trim(),
			args: values.arguments.split(/\r?\n/).filter((argument) => argument.length > 0),
			environment_handles: environmentHandles,
			working_directory_uri: session.workspace_uri,
			rows: Number(values.rows),
			cols: Number(values.cols),
			max_runtime_seconds: Number(values.runtime)
		};
		const preview = await api("/v1/background-terminals/preview", {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				terminal
			})
		});
		if (!await requestAction({
			eyebrow: "Permission preview",
			title: "Start this background terminal?",
			description: "Review the exact effective permissions. Credentials are referenced by handle and never shown or persisted in this preview.",
			details: [...preview.permissions.map((permission) => `${permission.kind}: ${permission.value} — ${permission.reason}`), `Confirmation digest: ${preview.permissions_sha256}`],
			confirm: "Confirm and start"
		})) return;
		const started = await api("/v1/background-terminals", {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				terminal,
				confirmation: {
					confirmed: true,
					permissions_sha256: preview.permissions_sha256
				}
			})
		});
		await refreshTeam();
		toast(`Background terminal started · ${started.id}`);
	}
	function renderGoals(goals, runs = []) {
		const container = $("team-goals");
		container.replaceChildren();
		container.className = "team-queue";
		const active = goals.filter((goal) => !["achieved", "cancelled"].includes(goal.status));
		if (!active.length) {
			container.textContent = "No active goals";
			container.classList.add("empty");
			return;
		}
		active.forEach((goal) => {
			const run = runs.filter((candidate) => candidate.goal_id === goal.id).sort((left, right) => String(right.updated_at).localeCompare(String(left.updated_at)))[0];
			const row = document.createElement("div");
			row.className = "task";
			const title = document.createElement("div");
			title.className = "task-title";
			title.textContent = goal.title;
			const meta = document.createElement("div");
			meta.className = "task-meta";
			meta.textContent = `${goal.status}${run ? ` · run ${run.status}` : ""} · ${goal.outcome_definition}`;
			const actions = document.createElement("div");
			actions.className = "task-buttons";
			if (!run || run.status === "cancelled") {
				const continueButton = document.createElement("button");
				continueButton.type = "button";
				continueButton.textContent = "Continue automatically";
				continueButton.addEventListener("click", () => {
					continueGoal(goal).catch((error) => toast(error.message));
				});
				actions.append(continueButton);
			} else {
				if (run.status === "active") {
					const pause = document.createElement("button");
					pause.type = "button";
					pause.textContent = "Pause run";
					pause.addEventListener("click", () => {
						controlGoalRun(goal, run, "paused").catch((error) => toast(error.message));
					});
					actions.append(pause);
				}
				if (run.status === "paused" || run.status === "awaiting_verification") {
					const resume = document.createElement("button");
					resume.type = "button";
					resume.textContent = "Resume run";
					resume.addEventListener("click", () => {
						controlGoalRun(goal, run, "active").catch((error) => toast(error.message));
					});
					actions.append(resume);
				}
				const cancel = document.createElement("button");
				cancel.type = "button";
				cancel.textContent = "Cancel run";
				cancel.addEventListener("click", () => {
					controlGoalRun(goal, run, "cancelled").catch((error) => toast(error.message));
				});
				actions.append(cancel);
			}
			[["Achieve", "achieved"], ["Cancel", "cancelled"]].forEach(([label, status]) => {
				const button = document.createElement("button");
				button.textContent = label;
				button.addEventListener("click", () => updateGoal(goal, status));
				actions.append(button);
			});
			row.append(title, meta, actions);
			container.append(row);
		});
	}
	async function controlGoalRun(goal, run, status) {
		if (status === "cancelled") {
			if (!await requestAction({
				eyebrow: "Goal automation",
				title: "Cancel this Goal run?",
				description: "No further Tasks will start. Active Agent work receives a safe cancellation request; completed evidence remains available.",
				details: [`Goal: ${goal.title}`, `Run: ${run.id}`],
				confirm: "Cancel run",
				danger: true
			})) return;
		}
		await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/goals/${encodeURIComponent(goal.id)}/runs/${encodeURIComponent(run.id)}`, {
			method: "PATCH",
			body: JSON.stringify({
				scope: scope(),
				status,
				expected_revision: run.revision
			})
		});
		await refreshTeam();
		toast(status === "active" ? "Goal run resumed" : status === "paused" ? "Goal run will pause after the current Task" : "Goal run cancelled");
	}
	async function continueGoal(goal) {
		const workspace = $("workspace").value.trim();
		const model = $("model").value.trim();
		if (!workspace || !model) {
			toast("Set a workspace and model before continuing a Goal.");
			openDrawer("settings-drawer");
			return;
		}
		const values = await requestAction({
			eyebrow: "Goal automation",
			title: "Continue ready Tasks automatically",
			description: "Runs one ready unblocked Task at a time. Each result stops in Review; a verified Team Outcome is still required before the Goal can be achieved.",
			details: [
				`Goal: ${goal.title}`,
				"WIP and Agent concurrency remain enforced.",
				"Each Task reserves its maximum model budget before it starts."
			],
			confirm: "Start Goal run",
			fields: [{
				name: "attempts",
				label: "Maximum attempts per Task",
				type: "number",
				min: 1,
				value: 2,
				required: true
			}, {
				name: "modelBudget",
				label: "Model budget per Task (USD)",
				type: "number",
				min: 0,
				value: 5,
				required: true
			}]
		});
		if (!values) return;
		const continuation = await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/goals/${encodeURIComponent(goal.id)}/continue`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				workspace_uri: workspace,
				model,
				idempotency_key: `web-goal-${crypto.randomUUID()}`,
				max_attempts: Number(values.attempts),
				max_runtime_seconds: 3600,
				max_cost_micros: Math.round(Number(values.modelBudget) * 1e6)
			})
		});
		await refreshSessions();
		await refreshTeam();
		toast(`Goal run started · ${continuation.task.title}`);
	}
	async function updateGoal(goal, status) {
		await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/goals/${encodeURIComponent(goal.id)}`, {
			method: "PATCH",
			body: JSON.stringify({
				scope: scope(),
				title: goal.title,
				outcome_definition: goal.outcome_definition,
				status,
				target_date: goal.target_date
			})
		});
		await refreshTeam();
	}
	function renderOwnership(items) {
		const resources = $("team-resources");
		resources.replaceChildren();
		resources.className = "team-queue";
		if (!items.length) {
			resources.textContent = "No owned resources";
			resources.classList.add("empty");
			return;
		}
		items.forEach((item) => {
			const row = document.createElement("div");
			row.className = "task";
			const title = document.createElement("div");
			title.className = "task-title";
			title.textContent = `${item.resource_type}: ${item.resource_uri}`;
			const meta = document.createElement("div");
			meta.className = "task-meta";
			meta.textContent = [item.service_tier, item.on_call].filter(Boolean).join(" · ") || "Team owned";
			const actions = document.createElement("div");
			actions.className = "task-buttons";
			const transfer = document.createElement("button");
			transfer.textContent = "Transfer";
			transfer.addEventListener("click", () => updateOwnership(item, false));
			const archive = document.createElement("button");
			archive.textContent = "Archive";
			archive.addEventListener("click", () => updateOwnership(item, true));
			actions.append(transfer, archive);
			row.append(title, meta, actions);
			resources.append(row);
		});
	}
	async function updateOwnership(item, archived) {
		const values = await requestAction(archived ? {
			eyebrow: "Ownership",
			title: "Archive ownership",
			description: `Remove ${item.resource_uri} from active Team ownership?`,
			confirm: "Archive",
			danger: true
		} : {
			eyebrow: "Ownership",
			title: "Transfer ownership",
			description: `Move ${item.resource_uri} to another Team.`,
			confirm: "Transfer",
			fields: [{
				name: "team",
				label: "Destination Team ID",
				value: scope().team_id,
				required: true
			}]
		});
		if (!values) return;
		const newTeam = archived ? null : values.team;
		await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/ownership/${encodeURIComponent(item.id)}`, {
			method: "PATCH",
			body: JSON.stringify({
				scope: scope(),
				service_tier: item.service_tier,
				on_call: item.on_call,
				new_team_id: newTeam,
				archived
			})
		});
		await refreshTeam();
		toast(archived ? "Ownership archived" : "Ownership transferred");
	}
	async function setCapacity() {
		const values = await requestAction({
			eyebrow: "Team limits",
			title: "Set capacity",
			description: "Bound concurrent work so the queue stays legible and predictable.",
			confirm: "Save capacity",
			fields: [
				{
					name: "human",
					label: "Human available hours",
					type: "number",
					min: 0,
					value: 40,
					required: true
				},
				{
					name: "agents",
					label: "Maximum concurrent agents",
					type: "number",
					min: 0,
					value: 4,
					required: true
				},
				{
					name: "wip",
					label: "Team work-in-progress limit",
					type: "number",
					min: 1,
					value: 6,
					required: true
				}
			]
		});
		if (!values) return;
		const human = Number(values.human);
		const agents = Number(values.agents);
		const wip = Number(values.wip);
		if (![
			human,
			agents,
			wip
		].every(Number.isFinite)) return;
		await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/capacity`, {
			method: "PUT",
			body: JSON.stringify({
				scope: scope(),
				human_available_hours: human,
				agent_concurrency: agents,
				wip_limit: wip
			})
		});
		await refreshTeam();
		toast("Team capacity saved");
	}
	async function createOwnership() {
		const values = await requestAction({
			eyebrow: "Ownership",
			title: "Add owned resource",
			description: "Make responsibility visible in the same place the Team works.",
			confirm: "Add ownership",
			fields: [
				{
					name: "resourceType",
					label: "Resource type",
					options: [
						["repository", "Repository"],
						["service", "Service"],
						["environment", "Environment"]
					]
				},
				{
					name: "resourceUri",
					label: "Absolute resource URI",
					placeholder: "https://github.example/org/repo",
					required: true
				},
				{
					name: "serviceTier",
					label: "Service tier (optional)",
					placeholder: "tier-1"
				},
				{
					name: "onCall",
					label: "On-call URI (optional)",
					placeholder: "https://oncall.example/team"
				}
			]
		});
		if (!values) return;
		const { resourceType, resourceUri } = values;
		const serviceTier = values.serviceTier || null;
		const onCall = values.onCall || null;
		await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/ownership`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				resource_type: resourceType,
				resource_uri: resourceUri,
				service_tier: serviceTier,
				on_call: onCall
			})
		});
		await refreshTeam();
		toast("Ownership added");
	}
	async function createBudget() {
		const values = await requestAction({
			eyebrow: "Spend guardrail",
			title: "Set 30-day budget",
			description: "A hard limit protects the Team without adding choices to every task.",
			confirm: "Set budget",
			fields: [{
				name: "model",
				label: "Model budget (USD)",
				type: "number",
				min: 0,
				value: 1e3,
				required: true
			}, {
				name: "runner",
				label: "Runner budget (USD)",
				type: "number",
				min: 0,
				value: 500,
				required: true
			}]
		});
		if (!values) return;
		const modelDollars = Number(values.model);
		const runnerDollars = Number(values.runner);
		if (![modelDollars, runnerDollars].every((value) => Number.isFinite(value) && value >= 0)) return;
		const start = /* @__PURE__ */ new Date();
		const end = new Date(start.getTime() + 30 * 86400 * 1e3);
		await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/budgets`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				period_start: start.toISOString(),
				period_end: end.toISOString(),
				model_limit_micros: Math.round(modelDollars * 1e6),
				runner_limit_micros: Math.round(runnerDollars * 1e6),
				hard_limit: true
			})
		});
		await refreshTeam();
		toast("Budget guardrail set");
	}
	function renderTasks(tasks) {
		const queue = $("team-queue");
		queue.replaceChildren();
		queue.className = "team-queue";
		const activeTasks = tasks.filter((task) => !["verified", "cancelled"].includes(task.status));
		if (!activeTasks.length) {
			queue.textContent = "No queued work";
			queue.classList.add("empty");
			return;
		}
		activeTasks.forEach((task) => {
			const row = document.createElement("div");
			row.className = "task";
			const title = document.createElement("div");
			title.className = "task-title";
			title.textContent = task.title;
			const meta = document.createElement("div");
			meta.className = "task-meta";
			meta.textContent = `${task.status} · P${task.priority} · ${task.source}`;
			const actions = document.createElement("div");
			actions.className = "task-buttons";
			const open = document.createElement("button");
			open.type = "button";
			open.textContent = "Open in Chat";
			open.addEventListener("click", () => openTeamTaskInChat(task));
			actions.append(open);
			(task.status === "ready" ? ["in_progress", "blocked"] : task.status === "in_progress" ? ["review", "blocked"] : task.status === "blocked" ? ["ready", "in_progress"] : task.status === "review" ? ["in_progress"] : []).forEach((status) => {
				const button = document.createElement("button");
				button.textContent = status.replace("_", " ");
				button.addEventListener("click", () => updateTask(task, status));
				actions.append(button);
			});
			if (task.status === "review") {
				const verify = document.createElement("button");
				verify.textContent = "verify outcome";
				verify.addEventListener("click", () => verifyOutcome(task));
				actions.append(verify);
			}
			row.append(title, meta, actions);
			queue.append(row);
		});
	}
	async function openTeamTaskInChat(task) {
		const existing = state.sessions.find((session) => session.scope?.task_id === task.id);
		if (existing) await selectSession(existing);
		else {
			const workspace = $("workspace").value.trim();
			const model = $("model").value.trim();
			if (!workspace || !model) {
				toast("Set a workspace and model before opening Team work in Chat.");
				openDrawer("settings-drawer");
				return;
			}
			const session = await api("/v1/sessions", {
				method: "POST",
				body: JSON.stringify({
					scope: {
						...scope(),
						goal_id: task.goal_id,
						task_id: task.id
					},
					workspace_uri: workspace,
					title: task.title,
					model
				})
			});
			await refreshSessions();
			await selectSession(session);
		}
		const acceptance = task.acceptance_criteria.length ? `\nAcceptance criteria:\n${task.acceptance_criteria.map((item) => `- ${item}`).join("\n")}` : "";
		const evidence = task.required_evidence.length ? `\nRequired evidence:\n${task.required_evidence.map((item) => `- ${item}`).join("\n")}` : "";
		$("prompt").value = `Work on Team task: ${task.title}${acceptance}${evidence}`;
		sessionStorage.setItem(composerTextDraftKey(), $("prompt").value);
		resizePrompt();
		$("prompt").focus();
		announce(`Team task ${task.title} opened in Chat`);
	}
	async function createGoal() {
		const values = await requestAction({
			eyebrow: "Team outcome",
			title: "Create a goal",
			description: "Define the observable result before creating work.",
			confirm: "Create goal",
			fields: [{
				name: "title",
				label: "Goal title",
				placeholder: "Ship passwordless sign-in",
				required: true
			}, {
				name: "outcome",
				label: "Observable outcome",
				placeholder: "All supported clients sign in without passwords and the rollout has no P0 regressions.",
				multiline: true,
				required: true
			}]
		});
		if (!values) return;
		const { title, outcome } = values;
		await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/goals`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				title,
				outcome_definition: outcome,
				target_date: null
			})
		});
		await refreshTeam();
		toast("Goal created");
	}
	async function createTask() {
		const values = await requestAction({
			eyebrow: "Team work",
			title: "Create a task",
			description: "Capture the work source and the evidence required to call it done.",
			confirm: "Create task",
			fields: [
				{
					name: "title",
					label: "Task title",
					placeholder: "Fix token refresh race",
					required: true
				},
				{
					name: "source",
					label: "Source",
					options: [
						["manual", "Manual"],
						["issue", "Issue"],
						["review", "Review"],
						["ci", "CI"],
						["security", "Security"],
						["incident", "Incident"]
					]
				},
				{
					name: "evidence",
					label: "Required evidence (comma separated)",
					value: "tests,review",
					required: true
				}
			]
		});
		if (!values) return;
		const { title, source } = values;
		const goalId = (await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/goals?${teamQuery()}`))[0]?.id || null;
		const evidence = values.evidence.split(",").map((item) => item.trim()).filter(Boolean);
		await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/tasks`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				goal_id: goalId,
				source,
				title,
				priority: 50,
				assignee_type: null,
				assignee_id: null,
				acceptance_criteria: [],
				required_evidence: evidence
			})
		});
		await refreshTeam();
		toast("Task created");
	}
	async function updateTask(task, status) {
		let blockers = [];
		if (status === "blocked") {
			const values = await requestAction({
				eyebrow: "Team work",
				title: "Mark task blocked",
				description: "Name the constraint so another person or agent can act on it.",
				confirm: "Mark blocked",
				fields: [{
					name: "blocker",
					label: "Blocking condition",
					multiline: true,
					required: true
				}]
			});
			if (!values) return;
			blockers = [values.blocker];
		}
		await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/tasks/${encodeURIComponent(task.id)}`, {
			method: "PATCH",
			body: JSON.stringify({
				scope: scope(),
				status,
				assignee_type: task.assignee_type,
				assignee_id: task.assignee_id,
				blockers
			})
		});
		await refreshTeam();
		toast(`Task moved to ${status.replace("_", " ")}`);
	}
	async function verifyOutcome(task) {
		if (!task.goal_id) {
			addActivity("outcome.error", { error: "Task has no goal" });
			return;
		}
		const now = (/* @__PURE__ */ new Date()).toISOString();
		const evidence = task.required_evidence.map((kind) => ({
			kind,
			uri: `manual-review://${encodeURIComponent(task.id)}/${encodeURIComponent(kind)}`,
			result: "passed",
			collected_at: now
		}));
		const values = await requestAction({
			eyebrow: "Verification",
			title: "Verify outcome",
			description: `Record the evidence for “${task.title}”.`,
			confirm: "Verify outcome",
			fields: [{
				name: "pr",
				label: "Pull request URL (optional)",
				type: "url",
				placeholder: "https://github.example/org/repo/pull/123"
			}]
		});
		if (!values) return;
		const pr = values.pr || null;
		await api(`/v1/teams/${encodeURIComponent(scope().team_id)}/outcomes`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				goal_id: task.goal_id,
				task_id: task.id,
				evidence,
				pull_request_url: pr
			})
		});
		await refreshTeam();
		toast("Outcome verified");
	}
	return {
		createBackgroundTask,
		createBackgroundTerminal,
		createBudget,
		createGoal,
		createOwnership,
		createTask,
		exportTeamAudit,
		refreshTeam,
		renderAgentRuns,
		renderBackgroundTerminals,
		renderBudgets,
		renderCapacity,
		renderDurableTasks,
		renderGoals,
		renderOwnership,
		renderTasks,
		renderTeamApprovals,
		renderTeamAudit,
		renderTeamGovernance,
		renderTeamOutcomes,
		setCapacity
	};
}
//#endregion
//#region src/pages/workspace-library.ts
function createWorkspaceLibrary(context) {
	const { lookup: $, api, state, catalogQuery, isCurrent, selectSession, closeDrawers, closeUserMenu, routePath, workspaceName, copyText, toast, announce, appendMarkdownBlocks, createCodeBlock, renderReviewReport } = context;
	let artifactEntries = [];
	let artifactNextCursor = null;
	let selectedArtifactId = null;
	function projectGroups() {
		const byWorkspace = /* @__PURE__ */ new Map();
		state.sessions.forEach((session) => {
			const workspace = session.workspace_uri || "No workspace";
			const group = byWorkspace.get(workspace) || [];
			group.push(session);
			byWorkspace.set(workspace, group);
		});
		return [...byWorkspace.entries()].map(([workspace, sessions]) => ({
			id: sessions[0].id,
			workspace,
			sessions: sessions.sort((left, right) => String(right.updated_at || "").localeCompare(String(left.updated_at || "")))
		})).sort((left, right) => workspaceName(left.workspace).localeCompare(workspaceName(right.workspace)));
	}
	async function renderProjects(selectedProjectId = null) {
		const groups = projectGroups();
		const selected = groups.find((group) => group.id === selectedProjectId) || groups[0] || null;
		const list = $("project-list");
		list.replaceChildren();
		groups.forEach((group) => {
			const button = document.createElement("button");
			button.type = "button";
			button.classList.toggle("active", group === selected);
			if (group === selected) button.setAttribute("aria-current", "page");
			const title = document.createElement("strong");
			title.textContent = workspaceName(group.workspace);
			const detail = document.createElement("small");
			detail.textContent = `${group.sessions.length} session${group.sessions.length === 1 ? "" : "s"} · ${group.workspace}`;
			button.append(title, detail);
			button.addEventListener("click", () => showProjects(group.id));
			list.append(button);
		});
		const detail = $("project-detail");
		detail.replaceChildren();
		if (!selected) {
			detail.className = "project-detail empty";
			detail.textContent = "No projects yet.";
			return;
		}
		detail.className = "project-detail";
		const title = document.createElement("h3");
		title.textContent = workspaceName(selected.workspace);
		const uri = document.createElement("p");
		uri.textContent = selected.workspace;
		const contextHeading = document.createElement("strong");
		contextHeading.textContent = "Context sources";
		const context = document.createElement("div");
		context.className = "project-context";
		context.textContent = "Loading context…";
		const sessionsHeading = document.createElement("strong");
		sessionsHeading.textContent = "Recent sessions";
		const sessions = document.createElement("div");
		sessions.className = "project-sessions";
		selected.sessions.forEach((session) => {
			const button = document.createElement("button");
			button.type = "button";
			button.className = "project-session";
			const name = document.createElement("span");
			name.textContent = session.title;
			const meta = document.createElement("small");
			meta.textContent = `${session.status} · ${session.model}`;
			button.append(name, meta);
			button.addEventListener("click", () => selectSession(session));
			sessions.append(button);
		});
		detail.append(title, uri, contextHeading, context, sessionsHeading, sessions);
		if (!state.connected) {
			context.textContent = "Connect to inspect AGENTS.md, rules, memory, and token usage.";
			return;
		}
		const generation = state.generation;
		try {
			const summary = await api(`/v1/sessions/${encodeURIComponent(selected.sessions[0].id)}/context?${catalogQuery()}`);
			if (!isCurrent(generation) || $("project-detail") !== detail) return;
			context.replaceChildren();
			if (!summary.items.length) context.textContent = "No project instructions, memory, attachments, or IDE context.";
			else summary.items.forEach((item) => {
				const row = document.createElement("article");
				const source = document.createElement("span");
				source.textContent = item.source_uri;
				const meta = document.createElement("small");
				meta.textContent = `${String(item.kind).replaceAll("_", " ")} · ${item.estimated_tokens} tokens · ${item.trust_level}`;
				row.append(source, meta);
				context.append(row);
			});
		} catch (error) {
			if (isCurrent(generation)) context.textContent = `Context unavailable: ${error.message}`;
		}
	}
	function showProjects(projectId = null, { replace = false, updateRoute = true } = {}) {
		closeDrawers(false);
		document.body.classList.add("team-mode");
		$("team-view").hidden = true;
		$("projects-view").hidden = false;
		$("artifacts-view").hidden = true;
		$("extensions-view").hidden = true;
		$("workspace-shell").hidden = true;
		$("open-team").classList.remove("active");
		$("open-team").removeAttribute("aria-current");
		document.body.classList.remove("mobile-sidebar-open");
		closeUserMenu();
		const selected = projectGroups().find((group) => group.id === projectId)?.id || projectGroups()[0]?.id || null;
		if (updateRoute) routePath(projectRoute(selected), replace);
		renderProjects(selected).catch((error) => toast(error.message));
		$("projects-title").focus({ preventScroll: true });
	}
	function artifactContentText(artifact) {
		return typeof artifact.content === "string" ? artifact.content : JSON.stringify(artifact.content, null, 2);
	}
	function artifactFileName(artifact) {
		const safe = artifact.metadata.title.normalize("NFKC").replace(/[^\p{L}\p{N}._-]+/gu, "-").replace(/^-+|-+$/g, "").slice(0, 80) || "artifact";
		const extension = artifact.metadata.media_type === "text/markdown" ? "md" : artifact.metadata.media_type.includes("json") ? "json" : artifact.metadata.media_type.startsWith("image/") ? artifact.metadata.media_type.slice(6) : artifact.metadata.media_type === "application/pdf" ? "pdf" : "txt";
		return safe.includes(".") ? safe : `${safe}.${extension}`;
	}
	function downloadArtifact(artifact) {
		const blob = new Blob([artifactContentText(artifact)], { type: `${artifact.metadata.media_type};charset=utf-8` });
		const href = URL.createObjectURL(blob);
		const link = document.createElement("a");
		link.href = href;
		link.download = artifactFileName(artifact);
		link.click();
		URL.revokeObjectURL(href);
	}
	function renderArtifactLibraryContent(target, artifact) {
		target.replaceChildren();
		if (artifact.metadata.media_type === "application/vnd.opencoding.review+json") {
			renderReviewReport(target, artifact.content);
			return;
		}
		if (artifact.metadata.media_type === "text/markdown" && typeof artifact.content === "string") {
			const body = document.createElement("div");
			body.className = "message-body";
			appendMarkdownBlocks(body, artifact.content);
			target.append(body);
			return;
		}
		if (artifact.metadata.media_type.startsWith("image/") && typeof artifact.content === "string" && artifact.content.startsWith(`data:${artifact.metadata.media_type};base64,`)) {
			const image = document.createElement("img");
			image.src = artifact.content;
			image.alt = artifact.metadata.title;
			target.append(image);
			return;
		}
		target.append(createCodeBlock(artifactContentText(artifact), artifact.metadata.media_type.includes("json") ? "json" : "text"));
	}
	async function showArtifactDetail(artifactId, entry = null) {
		const generation = state.generation;
		selectedArtifactId = artifactId;
		renderArtifactList();
		const target = $("artifact-detail");
		target.replaceChildren();
		const loading = document.createElement("p");
		loading.className = "empty";
		loading.textContent = "Loading artifact…";
		target.append(loading);
		try {
			const artifact = await api(`/v1/artifacts/${encodeURIComponent(artifactId)}?${catalogQuery()}`);
			if (!isCurrent(generation) || $("artifacts-view").hidden || selectedArtifactId !== artifactId) return;
			const source = entry || artifactEntries.find((candidate) => candidate.metadata.id === artifactId) || null;
			target.replaceChildren();
			const heading = document.createElement("div");
			heading.className = "artifact-library-heading";
			const title = document.createElement("h3");
			title.textContent = artifact.metadata.title;
			const meta = document.createElement("p");
			meta.textContent = [
				artifact.metadata.media_type,
				source?.session_title || artifact.metadata.session_id,
				source?.workspace_uri,
				new Date(artifact.metadata.created_at).toLocaleString()
			].filter(Boolean).join(" · ");
			heading.append(title, meta);
			const actions = document.createElement("div");
			actions.className = "artifact-library-actions";
			const copy = document.createElement("button");
			copy.type = "button";
			copy.textContent = "Copy";
			copy.addEventListener("click", () => copyText(artifactContentText(artifact), "Artifact copied"));
			const download = document.createElement("button");
			download.type = "button";
			download.textContent = "Download";
			download.addEventListener("click", () => downloadArtifact(artifact));
			actions.append(copy, download);
			const sourceSession = state.sessions.find((session) => session.id === artifact.metadata.session_id);
			if (sourceSession) {
				const openSession = document.createElement("button");
				openSession.type = "button";
				openSession.textContent = "Open source session";
				openSession.addEventListener("click", () => {
					selectSession(sourceSession).catch((error) => toast(error.message));
				});
				actions.prepend(openSession);
			}
			const content = document.createElement("div");
			content.className = "artifact-library-content";
			renderArtifactLibraryContent(content, artifact);
			target.append(heading, actions, content);
			announce(`Opened artifact ${artifact.metadata.title}`);
		} catch (error) {
			if (isCurrent(generation) && selectedArtifactId === artifactId) target.textContent = `Artifact unavailable: ${error.message}`;
		}
	}
	function renderArtifactList() {
		const kind = $("artifact-filter").value;
		const visible = artifactEntries.filter((entry) => kind === "all" || entry.kind === kind);
		const list = $("artifact-list");
		list.replaceChildren();
		list.classList.toggle("empty", !visible.length);
		$("artifact-count").textContent = `${visible.length} result${visible.length === 1 ? "" : "s"}${artifactNextCursor ? " loaded" : ""}`;
		if (!visible.length) list.textContent = artifactEntries.length ? "No loaded artifacts match this type." : "No artifacts yet.";
		visible.forEach((entry) => {
			const button = document.createElement("button");
			button.type = "button";
			button.className = "artifact-row";
			button.classList.toggle("active", entry.metadata.id === selectedArtifactId);
			const heading = document.createElement("span");
			const title = document.createElement("strong");
			title.textContent = entry.metadata.title;
			const badge = document.createElement("span");
			badge.className = "artifact-kind";
			badge.textContent = entry.kind.replaceAll("_", " ");
			heading.append(title, badge);
			const session = document.createElement("small");
			session.textContent = `${entry.session_title} · ${workspaceName(entry.workspace_uri)}`;
			const created = document.createElement("small");
			created.textContent = new Date(entry.metadata.created_at).toLocaleString();
			button.append(heading, session, created);
			button.addEventListener("click", () => {
				routePath(artifactRoute(entry.metadata.id));
				showArtifactDetail(entry.metadata.id, entry).catch((error) => toast(error.message));
			});
			list.append(button);
		});
		$("load-more-artifacts").hidden = !artifactNextCursor;
	}
	async function loadArtifactPage(reset = false, requestedId = null) {
		if (!state.connected) {
			artifactEntries = [];
			artifactNextCursor = null;
			renderArtifactList();
			$("artifact-detail").textContent = "Connect to inspect artifacts.";
			return;
		}
		if (reset) {
			artifactEntries = [];
			artifactNextCursor = null;
		}
		const query = new URLSearchParams(catalogQuery());
		query.set("limit", "50");
		if (!reset && artifactNextCursor) query.set("cursor", artifactNextCursor);
		const page = await api(`/v1/artifacts?${query}`);
		const known = new Set(artifactEntries.map((entry) => entry.metadata.id));
		artifactEntries.push(...page.artifacts.filter((entry) => !known.has(entry.metadata.id)));
		artifactNextCursor = page.next_cursor;
		renderArtifactList();
		const selected = requestedId ? artifactEntries.find((entry) => entry.metadata.id === requestedId) || null : artifactEntries[0] || null;
		if (requestedId || selected) {
			const artifactId = requestedId || selected.metadata.id;
			if (!requestedId) routePath(artifactRoute(artifactId), true);
			await showArtifactDetail(artifactId, selected);
		} else {
			selectedArtifactId = null;
			const empty = document.createElement("p");
			empty.className = "empty";
			empty.textContent = "Select an artifact to inspect it.";
			$("artifact-detail").replaceChildren(empty);
		}
	}
	function showArtifacts(artifactId = null, { replace = false, updateRoute = true } = {}) {
		closeDrawers(false);
		document.body.classList.add("team-mode");
		$("team-view").hidden = true;
		$("projects-view").hidden = true;
		$("artifacts-view").hidden = false;
		$("extensions-view").hidden = true;
		$("workspace-shell").hidden = true;
		$("open-team").classList.remove("active");
		$("open-team").removeAttribute("aria-current");
		document.body.classList.remove("mobile-sidebar-open");
		closeUserMenu();
		selectedArtifactId = artifactId;
		if (updateRoute) routePath(artifactRoute(artifactId), replace);
		loadArtifactPage(true, artifactId).catch((error) => toast(error.message));
		$("artifacts-title").focus({ preventScroll: true });
	}
	const loadMoreArtifacts = () => loadArtifactPage(false, selectedArtifactId);
	return {
		loadArtifactPage,
		loadMoreArtifacts,
		renderArtifactList,
		renderProjects,
		showArtifacts,
		showProjects
	};
}
//#endregion
//#region src/main.ts
function isJsonObject(value) {
	return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
function $(id) {
	const element = document.getElementById(id);
	if (!element) throw new Error(`missing required UI element #${id}`);
	return element;
}
var state = {
	session: null,
	sessions: [],
	turn: null,
	turnRunning: false,
	pendingInputs: [],
	draftFiles: [],
	draftFilesByContext: /* @__PURE__ */ new Map(),
	after: 0,
	abort: null,
	reconnectTimer: null,
	approvals: /* @__PURE__ */ new Set(),
	questions: /* @__PURE__ */ new Set(),
	toolSteps: /* @__PURE__ */ new Map(),
	itemsById: /* @__PURE__ */ new Map(),
	capabilities: /* @__PURE__ */ new Set(),
	connected: false,
	connecting: false,
	authenticatedScope: null,
	generation: 0,
	permissionMode: "manual",
	assistantAlias: "Opencoding",
	usage: {
		input_tokens: 0,
		output_tokens: 0,
		total_tokens: 0,
		model_calls: 0,
		tool_calls: 0,
		turns: 0
	},
	usageTurns: /* @__PURE__ */ new Set(),
	goal: null,
	sideConversation: null
};
var transcriptProjection = emptyTranscriptProjection();
var loadedTranscriptSnapshot = null;
var TRANSCRIPT_WINDOW_SIZE = 600;
var transcriptWindowStart = 0;
var SESSION_WINDOW_SIZE = 100;
var sessionWindowStart = 0;
var fields = [
	"organization",
	"team",
	"actor",
	"workspace",
	"model",
	"title"
];
var identityFields = [
	"organization",
	"team",
	"actor"
];
var settingsDirty = false;
var actionResolver = null;
var commandSelection = 0;
var drawerReturnFocus = null;
var developmentInstanceId = null;
var developmentReloadTimer = null;
var mentionAbort = null;
var mentionTimer = null;
var mentionSelection = 0;
var contextPickerSelection = 0;
var contextPickerOptions = [];
var transcriptFollowing = true;
var composerSubmissionPending = false;
var clientPresence = [];
var presenceTimer = null;
var highlightClient = new HighlightClient();
var { appendBlocks: appendMarkdownBlocks, createCodeBlock, renderMessageContent } = createMarkdownRenderer({
	copyText,
	highlight: (code, language, resolve) => highlightClient.request(code, language, resolve)
});
var { addHook, addMcpServer, addSkill, loadExtensionCatalog, managePluginMarketplaces, renderExtensionList, showExtensions } = createExtensionsPage({
	lookup: $,
	api,
	state,
	scope,
	requestAction,
	toast,
	copyText,
	closeDrawers,
	closeUserMenu,
	catalogQuery,
	routePath
});
var { loadArtifactPage, loadMoreArtifacts, renderArtifactList, renderProjects, showArtifacts, showProjects } = createWorkspaceLibrary({
	lookup: $,
	api,
	state,
	catalogQuery,
	isCurrent,
	selectSession,
	closeDrawers,
	closeUserMenu,
	routePath,
	workspaceName,
	copyText,
	toast,
	announce,
	appendMarkdownBlocks,
	createCodeBlock,
	renderReviewReport
});
var { createBackgroundTask, createBackgroundTerminal, createBudget, createGoal, createOwnership, createTask, exportTeamAudit, refreshTeam, renderAgentRuns, renderBackgroundTerminals, renderBudgets, renderCapacity, renderDurableTasks, renderGoals, renderOwnership, renderTasks, renderTeamApprovals, renderTeamAudit, renderTeamGovernance, renderTeamOutcomes, setCapacity } = createTeamWorkPage({
	lookup: $,
	api,
	state,
	scope,
	catalogQuery,
	requestAction,
	toast,
	addActivity,
	selectSession,
	refreshSessions,
	openDrawer,
	announce,
	showArtifacts,
	showWorkspace,
	composerTextDraftKey,
	resizePrompt
});
var presenceClientId = (() => {
	const existing = sessionStorage.getItem("oc.client-presence-id");
	if (existing && /^[A-Za-z0-9:_-]{1,128}$/.test(existing)) return existing;
	const created = `web:${globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(16).slice(2)}`}`;
	sessionStorage.setItem("oc.client-presence-id", created);
	return created;
})();
var themeOrder = [
	"system",
	"light",
	"dark"
];
var permissionLabels = {
	manual: "Manual",
	accept_edits: "Accept edits",
	plan: "Plan"
};
function currentTheme() {
	const saved = localStorage.getItem("oc.theme");
	return saved && themeOrder.includes(saved) ? saved : "system";
}
function applyTheme(theme = currentTheme()) {
	document.documentElement.dataset.theme = theme;
	localStorage.setItem("oc.theme", theme);
	$("theme-toggle").textContent = `Theme: ${theme.charAt(0).toUpperCase()}${theme.slice(1)}`;
	const dark = theme === "dark" || theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches;
	document.querySelector("meta[name=\"theme-color\"]")?.setAttribute("content", dark ? "#1c1b19" : "#f7f6f2");
}
function closeUserMenu() {
	const menu = document.querySelector(".user-menu");
	if (menu) menu.open = false;
}
function routePath(path, replace = false) {
	if (window.location.pathname === path) return;
	window.history[replace ? "replaceState" : "pushState"]({}, "", path);
}
function showWorkspace({ replace = false, updateRoute = true } = {}) {
	document.body.classList.remove("team-mode");
	$("team-view").hidden = true;
	$("projects-view").hidden = true;
	$("artifacts-view").hidden = true;
	$("extensions-view").hidden = true;
	$("workspace-shell").hidden = false;
	$("open-team").classList.remove("active");
	$("open-team").removeAttribute("aria-current");
	closeUserMenu();
	if (updateRoute) routePath(sessionRoute(state.session?.id || null), replace);
}
var teamSectionTargets = {
	overview: "team-panel",
	work: "team-work-page",
	goals: "team-goals-page",
	agents: "team-agents-page",
	capacity: "team-capacity-page",
	ownership: "team-ownership-page",
	budgets: "team-budgets-page",
	approvals: "team-approvals-page",
	outcomes: "team-outcomes-page",
	audit: "team-audit-page"
};
function selectTeamSection(section, scroll = "auto") {
	document.querySelectorAll(".team-tabs button").forEach((button) => {
		const selected = button.dataset.teamSection === section;
		button.classList.toggle("active", selected);
		if (selected) button.setAttribute("aria-current", "page");
		else button.removeAttribute("aria-current");
	});
	document.querySelectorAll(".team-page[data-team-page]").forEach((page) => {
		page.hidden = page.dataset.teamPage !== section;
	});
	$(teamSectionTargets[section]).scrollIntoView({
		behavior: scroll,
		block: "start"
	});
}
function showTeam({ replace = false, updateRoute = true, section = "overview" } = {}) {
	closeDrawers(false);
	document.body.classList.add("team-mode");
	$("team-view").hidden = false;
	$("projects-view").hidden = true;
	$("artifacts-view").hidden = true;
	$("extensions-view").hidden = true;
	$("workspace-shell").hidden = true;
	$("open-team").classList.add("active");
	$("open-team").setAttribute("aria-current", "page");
	document.body.classList.remove("mobile-sidebar-open");
	closeUserMenu();
	if (updateRoute) routePath(teamRoute(section), replace);
	if (state.connected) refreshTeam().catch((error) => toast(error.message));
	selectTeamSection(section);
	$("team-title").focus({ preventScroll: true });
}
async function restoreRoute() {
	const route = parseRoute(window.location.pathname);
	if (route.type === "team") {
		showTeam({
			replace: true,
			updateRoute: false,
			section: route.section
		});
		return;
	}
	if (route.type === "projects") {
		showProjects(null, {
			replace: true,
			updateRoute: false
		});
		return;
	}
	if (route.type === "project") {
		showProjects(route.projectId, {
			replace: true,
			updateRoute: false
		});
		return;
	}
	if (route.type === "artifacts") {
		showArtifacts(null, {
			replace: true,
			updateRoute: false
		});
		return;
	}
	if (route.type === "artifact") {
		showArtifacts(route.artifactId, {
			replace: true,
			updateRoute: false
		});
		return;
	}
	if (route.type === "extensions") {
		showExtensions({
			replace: true,
			updateRoute: false
		});
		return;
	}
	if (route.type === "session" && state.connected) {
		const session = state.sessions.find((candidate) => candidate.id === route.sessionId);
		if (session) {
			await selectSession(session, { updateRoute: false });
			return;
		}
	}
	showWorkspace({
		replace: true,
		updateRoute: false
	});
}
async function pollDevelopmentInstance() {
	try {
		const response = await fetch("/v1/health", {
			cache: "no-store",
			credentials: "same-origin"
		});
		if (!response.ok) return;
		const health = await response.json();
		if (!health.development_instance_id) return;
		if (developmentInstanceId && developmentInstanceId !== health.development_instance_id) {
			window.location.reload();
			return;
		}
		developmentInstanceId = health.development_instance_id;
	} catch (_) {}
}
function enableDevelopmentAutoReload() {
	const bootstrap = document.querySelector("meta[name=\"opencoding-bootstrap\"]")?.content || "";
	if (!bootstrap || bootstrap === "__OPENCODING_BOOTSTRAP__") return;
	pollDevelopmentInstance();
	developmentReloadTimer = window.setInterval(pollDevelopmentInstance, 750);
}
function workspaceName(uri) {
	const clean = String(uri || "").replace(/\/$/, "");
	if (!clean) return "No workspace";
	try {
		return decodeURIComponent(clean.split("/").filter(Boolean).pop() || clean);
	} catch (_) {
		return clean;
	}
}
function updateContextChips() {
	$("workspace-chip").textContent = state.connected ? workspaceName($("workspace").value.trim()) : "No workspace";
	$("model-chip").textContent = state.session?.model || $("model").value.trim() || "No model";
	$("permission-chip").textContent = permissionLabels[state.permissionMode];
}
function activeMention() {
	const prompt = $("prompt").value;
	const match = prompt.match(/(^|\s)@([^\s@]*)$/);
	if (!match) return null;
	return {
		start: prompt.length - match[0].length,
		prefix: match[1],
		query: match[2]
	};
}
function closeMentionMenu() {
	if (mentionAbort) mentionAbort.abort();
	mentionAbort = null;
	$("mention-menu").hidden = true;
	$("mention-menu").replaceChildren();
	mentionSelection = 0;
}
function selectMention(path) {
	const mention = activeMention();
	if (!mention) return;
	const prompt = $("prompt");
	prompt.value = `${prompt.value.slice(0, mention.start)}${mention.prefix}@${path} `;
	sessionStorage.setItem(composerTextDraftKey(), prompt.value);
	closeMentionMenu();
	resizePrompt();
	prompt.focus();
}
function updateMentionSelection(index) {
	const options = [...$("mention-menu").querySelectorAll("button")];
	if (!options.length) return;
	mentionSelection = (index + options.length) % options.length;
	options.forEach((option, optionIndex) => {
		option.setAttribute("aria-selected", String(optionIndex === mentionSelection));
	});
	options[mentionSelection].scrollIntoView({ block: "nearest" });
}
async function loadFileMentions() {
	const mention = activeMention();
	if (!mention || !state.session || !state.capabilities.has("workspace.fuzzy_search")) {
		closeMentionMenu();
		return;
	}
	if (mentionAbort) mentionAbort.abort();
	mentionAbort = new AbortController();
	const controller = mentionAbort;
	const selectedSession = state.session.id;
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id,
		query: mention.query,
		limit: "12"
	});
	try {
		const paths = await api(`/v1/sessions/${encodeURIComponent(selectedSession)}/files?${query}`, { signal: controller.signal });
		if (controller.signal.aborted || state.session?.id !== selectedSession) return;
		const menu = $("mention-menu");
		menu.replaceChildren();
		paths.forEach((item, index) => {
			const option = document.createElement("button");
			option.type = "button";
			option.setAttribute("role", "option");
			option.setAttribute("aria-selected", String(index === 0));
			option.textContent = item.path;
			option.addEventListener("click", () => selectMention(item.path));
			menu.append(option);
		});
		mentionSelection = 0;
		menu.hidden = paths.length === 0;
	} catch (error) {
		if (error.name !== "AbortError") closeMentionMenu();
	}
}
function scheduleFileMentions() {
	if (mentionTimer) clearTimeout(mentionTimer);
	mentionTimer = setTimeout(() => {
		mentionTimer = null;
		loadFileMentions();
	}, 100);
}
function updateConversationState(hasMessages = Boolean($("messages").children.length)) {
	$("conversation-view").classList.toggle("is-empty", !hasMessages);
	if (hasMessages) advanceTranscript();
}
function transcriptIsNearBottom() {
	const messages = $("messages");
	return messages.scrollHeight - messages.scrollTop - messages.clientHeight <= 96;
}
function advanceTranscript(force = false) {
	const messages = $("messages");
	if (force) transcriptFollowing = true;
	if (transcriptFollowing) {
		messages.scrollTop = messages.scrollHeight;
		$("jump-latest").hidden = true;
	} else $("jump-latest").hidden = false;
}
function isCurrent(generation) {
	return state.connected && generation === state.generation;
}
function resizePrompt() {
	const prompt = $("prompt");
	prompt.style.height = "auto";
	prompt.style.height = `${Math.min(prompt.scrollHeight, 180)}px`;
}
function composerDraftContext(session = state.session) {
	return session ? `session:${session.id}` : "new";
}
function composerTextDraftKey(session = state.session) {
	return `oc.prompt-draft:${composerDraftContext(session)}`;
}
function saveComposerDraft() {
	const key = composerDraftContext();
	const text = $("prompt").value;
	if (text) sessionStorage.setItem(composerTextDraftKey(), text);
	else sessionStorage.removeItem(composerTextDraftKey());
	state.draftFilesByContext.set(key, [...state.draftFiles]);
}
function restoreComposerDraft() {
	state.draftFiles = [...state.draftFilesByContext.get(composerDraftContext()) || []];
	$("prompt").value = sessionStorage.getItem(composerTextDraftKey()) || "";
	resizePrompt();
	renderDraftAttachments();
	updateSendAction();
}
function transferNewComposerDraft(session) {
	const files = [...state.draftFiles];
	state.draftFilesByContext.set("new", []);
	state.draftFilesByContext.set(composerDraftContext(session), files);
	const text = sessionStorage.getItem("oc.prompt-draft:new");
	if (text) sessionStorage.setItem(composerTextDraftKey(session), text);
	sessionStorage.removeItem("oc.prompt-draft:new");
	state.draftFiles = [];
	$("prompt").value = "";
}
function formatBytes(value) {
	if (value < 1024) return `${value} B`;
	if (value < 1024 * 1024) return `${Math.ceil(value / 1024)} KB`;
	return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}
function renderDraftAttachments() {
	const target = $("attachment-list");
	target.replaceChildren();
	target.hidden = state.draftFiles.length === 0;
	state.draftFiles.forEach((file, index) => {
		const chip = document.createElement("div");
		chip.className = "attachment-chip";
		const name = document.createElement("span");
		name.textContent = file.name;
		name.title = file.name;
		const size = document.createElement("small");
		size.textContent = formatBytes(file.size);
		const remove = document.createElement("button");
		remove.type = "button";
		remove.textContent = "×";
		remove.setAttribute("aria-label", `Remove ${file.name}`);
		remove.addEventListener("click", () => {
			state.draftFiles.splice(index, 1);
			state.draftFilesByContext.set(composerDraftContext(), [...state.draftFiles]);
			renderDraftAttachments();
			updateSendAction();
			$("prompt").focus();
		});
		chip.append(name, size, remove);
		target.append(chip);
	});
}
function addDraftFiles(files) {
	const incoming = [...files];
	if (!state.capabilities.has("composer.attachments.v1")) {
		toast("This server does not support attachments.");
		return;
	}
	for (const file of incoming) {
		if (!file.size) {
			toast(`${file.name} is empty.`);
			continue;
		}
		if (file.size > 5 * 1024 * 1024) {
			toast(`${file.name} exceeds the 5 MB limit.`);
			continue;
		}
		if (state.draftFiles.length >= 8) {
			toast("A message can contain at most eight files.");
			break;
		}
		state.draftFiles.push(file);
	}
	state.draftFilesByContext.set(composerDraftContext(), [...state.draftFiles]);
	renderDraftAttachments();
	updateSendAction();
}
function fileBase64(file) {
	return new Promise((resolve, reject) => {
		const reader = new FileReader();
		reader.addEventListener("error", () => reject(reader.error || /* @__PURE__ */ new Error(`Could not read ${file.name}`)));
		reader.addEventListener("load", () => {
			const value = String(reader.result || "");
			const separator = value.indexOf(",");
			if (separator < 0) reject(/* @__PURE__ */ new Error(`Could not encode ${file.name}`));
			else resolve(value.slice(separator + 1));
		});
		reader.readAsDataURL(file);
	});
}
async function deleteDraftAttachment(id) {
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	});
	await api(`/v1/attachments/${encodeURIComponent(id)}?${query}`, { method: "DELETE" });
}
async function uploadDraftAttachments(sessionId, files) {
	const uploaded = [];
	try {
		for (const file of files) uploaded.push(await api(`/v1/sessions/${encodeURIComponent(sessionId)}/attachments`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				file_name: file.name,
				media_type: file.type || "application/octet-stream",
				content_base64: await fileBase64(file)
			})
		}));
		return uploaded;
	} catch (error) {
		await Promise.allSettled(uploaded.map((attachment) => deleteDraftAttachment(attachment.id)));
		throw error;
	}
}
function announce(message) {
	$("announcer").textContent = "";
	window.setTimeout(() => {
		$("announcer").textContent = message;
	}, 20);
}
function toast(message) {
	const item = document.createElement("div");
	item.className = "toast";
	item.textContent = message;
	$("toast-region").append(item);
	window.setTimeout(() => item.remove(), 3200);
}
function notificationMode() {
	const value = localStorage.getItem("oc.notification-mode");
	return value === "background" || value === "always" ? value : "off";
}
function updateNotificationControls() {
	const supported = "Notification" in globalThis;
	const mode = notificationMode();
	$("notification-mode").value = mode;
	$("notification-mode").disabled = !supported;
	$("enable-notifications").disabled = !supported || Notification.permission === "granted";
	$("enable-notifications").textContent = !supported ? "Desktop notifications unavailable" : Notification.permission === "granted" ? "Desktop notifications enabled" : Notification.permission === "denied" ? "Notifications blocked in browser settings" : "Enable desktop notifications";
	$("notification-status").textContent = !supported ? "This browser does not expose desktop notifications." : Notification.permission === "granted" ? mode === "off" ? "Permission granted; notifications are currently off." : mode === "background" ? "You will be notified when this tab is in the background." : "You will be notified for decisions and task completion." : Notification.permission === "denied" ? "Permission is blocked. Change it in browser site settings." : "Desktop notifications are off until you explicitly enable them.";
}
async function enableNotifications() {
	if (!("Notification" in globalThis)) {
		updateNotificationControls();
		return;
	}
	if (await Notification.requestPermission() === "granted" && notificationMode() === "off") localStorage.setItem("oc.notification-mode", "background");
	updateNotificationControls();
}
function notifyUser(tag, message) {
	const mode = notificationMode();
	if (mode === "off" || !("Notification" in globalThis) || Notification.permission !== "granted" || mode === "background" && document.visibilityState === "visible") return false;
	new Notification("Opencoding", {
		body: message,
		tag
	});
	return true;
}
async function copyText(text, success = "Copied") {
	try {
		await navigator.clipboard.writeText(text);
	} catch (_) {
		const temporary = document.createElement("textarea");
		temporary.value = text;
		temporary.setAttribute("readonly", "");
		temporary.className = "sr-only";
		document.body.append(temporary);
		temporary.select();
		document.execCommand("copy");
		temporary.remove();
	}
	toast(success);
}
function closeActionDialog(value = null) {
	if ($("action-dialog").open) $("action-dialog").close();
	const resolve = actionResolver;
	actionResolver = null;
	if (resolve) resolve(value);
}
function requestAction({ eyebrow = "Team action", title, description, details = [], confirm = "Continue", danger = false, fields: requestedFields = [] }) {
	if (actionResolver) closeActionDialog(null);
	$("action-eyebrow").textContent = eyebrow;
	$("action-title").textContent = title;
	$("action-description").textContent = description || "";
	$("confirm-action").textContent = confirm;
	$("confirm-action").classList.toggle("danger", danger);
	$("confirm-action").classList.toggle("primary", !danger);
	const container = $("action-fields");
	container.replaceChildren();
	if (details.length) {
		const list = document.createElement("ul");
		list.className = "impact-list";
		details.forEach((detail) => {
			const item = document.createElement("li");
			item.textContent = detail;
			list.append(item);
		});
		container.append(list);
	}
	requestedFields.forEach((field) => {
		const label = document.createElement("label");
		label.textContent = field.label;
		const input = document.createElement(field.multiline ? "textarea" : field.options ? "select" : "input");
		input.name = field.name;
		input.id = `action-${field.name}`;
		input.required = Boolean(field.required);
		if (!field.multiline && !field.options) input.type = field.type || "text";
		if (field.placeholder) input.placeholder = field.placeholder;
		if (field.min !== void 0) input.min = String(field.min);
		if (field.max !== void 0) input.max = String(field.max);
		if (field.maxlength !== void 0) input.maxLength = Number(field.maxlength);
		if (field.options) field.options.forEach(([value, text]) => input.append(new Option(text, value)));
		if (field.value !== void 0) input.value = String(field.value);
		label.append(input);
		container.append(label);
	});
	$("action-dialog").showModal();
	window.setTimeout(() => container.querySelector("input, textarea, select")?.focus(), 0);
	return new Promise((resolve) => {
		actionResolver = resolve;
	});
}
function submitActionDialog(event) {
	event.preventDefault();
	if (!$("action-form").reportValidity()) return;
	closeActionDialog(Object.fromEntries([...new FormData($("action-form")).entries()].map(([key, value]) => [key, String(value)])));
}
function showShortcuts() {
	if (!$("shortcuts-dialog").open) {
		$("shortcuts-dialog").showModal();
		document.querySelector(".dialog-close")?.focus();
	}
}
function renderSessionGoal() {
	const goal = state.goal;
	const container = $("session-goal");
	container.hidden = !goal;
	if (!goal) {
		container.removeAttribute("data-status");
		$("session-goal-objective").textContent = "";
		$("session-goal-progress").textContent = "";
		return;
	}
	container.dataset.status = goal.status;
	$("session-goal-status").textContent = `Goal · ${goal.status}`;
	$("session-goal-objective").textContent = goal.objective;
	const tokens = goal.input_tokens + goal.output_tokens;
	$("session-goal-progress").textContent = goal.token_budget ? `${tokens.toLocaleString()} / ${goal.token_budget.toLocaleString()} tokens · ${goal.continuation_count} continuations` : `${tokens.toLocaleString()} tokens · ${goal.continuation_count} continuations${goal.blocked_reason ? ` · ${goal.blocked_reason}` : ""}`;
	$("toggle-session-goal").textContent = goal.status === "active" ? "Pause" : "Resume";
	$("toggle-session-goal").disabled = goal.status === "completed";
	$("edit-session-goal").disabled = goal.status === "completed";
}
async function loadSessionGoal(sessionId = state.session?.id) {
	if (!sessionId || !state.capabilities.has("session.goal.v1")) {
		state.goal = null;
		renderSessionGoal();
		return;
	}
	const goal = await api(`/v1/sessions/${encodeURIComponent(sessionId)}/goal?${catalogQuery()}`);
	if (state.session?.id !== sessionId) return;
	state.goal = goal;
	renderSessionGoal();
}
async function editSessionGoal() {
	if (!state.session) return;
	const current = state.goal;
	const values = await requestAction({
		eyebrow: "Persistent Goal",
		title: current ? "Edit Goal" : "Start a Goal",
		description: current ? "Update the outcome Opencoding should keep working toward." : "Opencoding will keep working across turns until this outcome is complete, paused, or genuinely blocked.",
		confirm: current ? "Save Goal" : "Start Goal",
		fields: [{
			name: "objective",
			label: "Objective",
			multiline: true,
			maxlength: 4e3,
			required: true,
			value: current?.objective || "",
			placeholder: "Describe the verified outcome you want"
		}]
	});
	if (!values) return;
	const sessionId = state.session.id;
	const objective = values.objective.trim();
	const goal = current ? await api(`/v1/sessions/${encodeURIComponent(sessionId)}/goal`, {
		method: "PATCH",
		body: JSON.stringify({
			scope: scope(),
			objective,
			status: null,
			auto_continue: null,
			expected_revision: current.revision,
			blocked_reason: null
		})
	}) : await api(`/v1/sessions/${encodeURIComponent(sessionId)}/goal`, {
		method: "PUT",
		body: JSON.stringify({
			scope: scope(),
			objective,
			auto_continue: true,
			token_budget: null
		})
	});
	if (state.session?.id !== sessionId) return;
	state.goal = goal;
	renderSessionGoal();
	toast(current ? "Goal updated" : "Goal started");
	if (!current && !state.turnRunning) await executeContent(objective);
}
async function toggleSessionGoal() {
	const session = state.session;
	const current = state.goal;
	if (!session || !current || current.status === "completed") return;
	const status = current.status === "active" ? "paused" : "active";
	const goal = await api(`/v1/sessions/${encodeURIComponent(session.id)}/goal`, {
		method: "PATCH",
		body: JSON.stringify({
			scope: scope(),
			objective: null,
			status,
			auto_continue: null,
			expected_revision: current.revision,
			blocked_reason: null
		})
	});
	if (state.session?.id !== session.id) return;
	state.goal = goal;
	renderSessionGoal();
	toast(status === "active" ? "Goal resumed" : "Goal paused");
}
async function clearSessionGoal() {
	const session = state.session;
	const current = state.goal;
	if (!session || !current) return;
	if (!await requestAction({
		eyebrow: "Persistent Goal",
		title: "Clear this Goal?",
		description: "Automatic continuation will stop. Existing conversation history is preserved.",
		confirm: "Clear Goal",
		danger: true
	})) return;
	await api(`/v1/sessions/${encodeURIComponent(session.id)}/goal`, {
		method: "DELETE",
		body: JSON.stringify({
			scope: scope(),
			expected_revision: current.revision
		})
	});
	if (state.session?.id !== session.id) return;
	state.goal = null;
	renderSessionGoal();
	toast("Goal cleared");
}
function commandDefinitions() {
	return [
		{
			label: "Focus task prompt",
			detail: "Return to the primary action",
			shortcut: "⌘L",
			run: () => $("prompt").focus()
		},
		{
			label: "Start a new task",
			detail: "Keep the workspace and clear the conversation",
			shortcut: "⌘N",
			run: () => {
				clearSessionSelection();
				showWorkspace();
			}
		},
		{
			label: state.goal ? "Manage persistent Goal" : "Start persistent Goal",
			detail: "Keep working across turns until a verified outcome is reached",
			shortcut: "",
			enabled: () => Boolean(state.session) && state.capabilities.has("session.goal.v1"),
			run: editSessionGoal
		},
		{
			label: "Show changes",
			detail: "Review the current Git diff when you need it",
			shortcut: "⌘D",
			enabled: () => Boolean(state.session),
			run: async () => {
				showWorkspace();
				openDrawer("inspector");
				await showDiff();
			}
		},
		{
			label: "Toggle history",
			detail: "Show or hide recent sessions",
			shortcut: "⌘B",
			run: toggleHistory
		},
		{
			label: "Search task history",
			detail: "Find a session by title, workspace, or model",
			shortcut: "⌘F",
			run: focusSessionSearch
		},
		{
			label: "Settings",
			detail: "Connection, identity, model, and workspace",
			shortcut: "",
			run: () => openDrawer("settings-drawer")
		},
		{
			label: "Open projects",
			detail: "Inspect workspace instructions, context, memory, and recent sessions",
			shortcut: "",
			run: () => showProjects()
		},
		{
			label: "Open artifacts",
			detail: "Browse reports, documents, diffs, and larger results across sessions",
			shortcut: "",
			run: () => showArtifacts()
		},
		{
			label: "Open extensions",
			detail: "Inspect MCP servers, tools, permissions, trust, and sources",
			shortcut: "",
			run: () => showExtensions()
		},
		{
			label: "Start background terminal",
			detail: "Run a confirmed PTY process that continues after this page closes",
			shortcut: "",
			enabled: () => state.capabilities.has("terminal.background_pty.v1"),
			run: createBackgroundTerminal
		},
		{
			label: "Show context",
			detail: "Inspect token usage, sources, AGENTS.md, and memory",
			shortcut: "",
			enabled: () => Boolean(state.session),
			run: async () => {
				openDrawer("inspector");
				await showContext();
			}
		},
		{
			label: "Start side conversation",
			detail: "Ask in a temporary Fork without changing the current Session",
			shortcut: "",
			enabled: () => Boolean(state.session) && !state.turnRunning && state.capabilities.has("session.side_conversation.v1"),
			run: createSideConversation
		},
		{
			label: "Open Team",
			detail: "Goals, ownership, capacity, budget, and work queue",
			shortcut: "",
			run: showTeam
		},
		{
			label: "Cycle theme",
			detail: "Switch among system, light, and dark",
			shortcut: "",
			run: cycleTheme
		},
		{
			label: "Show keyboard shortcuts",
			detail: "See every keyboard-first action",
			shortcut: "⌘/",
			run: showShortcuts
		}
	];
}
function cycleTheme() {
	const theme = currentTheme();
	applyTheme(themeOrder[(themeOrder.indexOf(theme) + 1) % themeOrder.length]);
}
function focusSessionSearch() {
	if (window.matchMedia("(max-width: 760px)").matches) document.body.classList.add("mobile-sidebar-open");
	else if (document.body.classList.contains("sidebar-collapsed")) toggleHistory();
	$("session-search").focus();
}
function renderCommands() {
	const query = $("command-query").value.trim().toLowerCase();
	const commands = commandDefinitions().filter((command) => !query || `${command.label} ${command.detail}`.toLowerCase().includes(query));
	commandSelection = Math.max(0, Math.min(commandSelection, commands.length - 1));
	const results = $("command-results");
	results.replaceChildren();
	commands.forEach((command, index) => {
		const button = document.createElement("button");
		button.type = "button";
		button.id = `command-option-${index}`;
		button.className = `command-item${index === commandSelection ? " selected" : ""}`;
		button.setAttribute("role", "option");
		button.setAttribute("aria-selected", String(index === commandSelection));
		button.disabled = command.enabled ? !command.enabled() : false;
		const text = document.createElement("span");
		const label = document.createElement("span");
		const detail = document.createElement("small");
		label.textContent = command.label;
		detail.textContent = command.detail;
		text.append(label, detail);
		button.append(text);
		if (command.shortcut) {
			const key = document.createElement("kbd");
			key.textContent = command.shortcut;
			button.append(key);
		}
		button.addEventListener("click", () => runCommand(command));
		results.append(button);
	});
	$("command-query").setAttribute("aria-activedescendant", commands.length ? `command-option-${commandSelection}` : "");
	return commands;
}
function runCommand(command) {
	$("command-dialog").close();
	Promise.resolve(command.run()).catch((error) => toast(error.message));
}
function openCommands() {
	commandSelection = 0;
	$("command-query").value = "";
	renderCommands();
	$("command-dialog").showModal();
	$("command-query").focus();
}
function filteredContextPickerOptions() {
	const query = $("context-picker-query").value.trim().toLowerCase();
	return contextPickerOptions.filter((option) => !query || `${option.label} ${option.detail} ${option.meta.join(" ")}`.toLowerCase().includes(query));
}
function renderContextPicker() {
	const options = filteredContextPickerOptions();
	contextPickerSelection = Math.max(0, Math.min(contextPickerSelection, options.length - 1));
	const target = $("context-picker-results");
	target.replaceChildren();
	if (!options.length) {
		const empty = document.createElement("p");
		empty.className = "command-hint";
		empty.textContent = "No matching options";
		target.append(empty);
	}
	options.forEach((option, index) => {
		const button = document.createElement("button");
		button.type = "button";
		button.id = `context-picker-option-${index}`;
		button.className = `command-item${index === contextPickerSelection ? " selected" : ""}${option.current ? " current" : ""}`;
		button.setAttribute("role", "option");
		button.setAttribute("aria-selected", String(index === contextPickerSelection));
		button.disabled = Boolean(option.disabled);
		if (option.disabledReason) button.title = option.disabledReason;
		const copy = document.createElement("span");
		const label = document.createElement("span");
		label.textContent = `${option.current ? "✓ " : ""}${option.label}`;
		const detail = document.createElement("small");
		detail.textContent = option.disabledReason || option.detail;
		copy.append(label, detail);
		const meta = document.createElement("span");
		meta.className = "picker-meta";
		option.meta.forEach((value) => {
			const chip = document.createElement("span");
			chip.textContent = value;
			meta.append(chip);
		});
		button.append(copy, meta);
		button.addEventListener("click", () => {
			Promise.resolve(option.select()).then(() => $("context-picker-dialog").close()).catch((error) => toast(error.message));
		});
		target.append(button);
	});
	$("context-picker-query").setAttribute("aria-activedescendant", options.length ? `context-picker-option-${contextPickerSelection}` : "");
	return options;
}
function openContextPicker({ eyebrow, title, placeholder, hint, options }) {
	contextPickerOptions = options;
	contextPickerSelection = Math.max(0, options.findIndex((option) => option.current));
	$("context-picker-eyebrow").textContent = eyebrow;
	$("context-picker-title").textContent = title;
	$("context-picker-query").placeholder = placeholder;
	$("context-picker-query").value = "";
	$("context-picker-hint").textContent = hint;
	renderContextPicker();
	$("context-picker-dialog").showModal();
	$("context-picker-query").focus();
}
function catalogQuery() {
	const s = scope();
	return new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	});
}
async function chooseModel(modelId) {
	if (state.session) {
		const updated = await api(`/v1/sessions/${encodeURIComponent(state.session.id)}`, {
			method: "PATCH",
			body: JSON.stringify({
				scope: scope(),
				model: modelId
			})
		});
		state.session = updated;
		state.sessions = state.sessions.map((session) => session.id === updated.id ? updated : session);
		$("session-meta").textContent = `${updated.status} · ${updated.model} · ${updated.workspace_uri}`;
		renderSessions();
	} else {
		$("model").value = modelId;
		await api("/v1/settings", {
			method: "PUT",
			body: JSON.stringify(daemonSettings())
		});
		settingsDirty = false;
	}
	updateContextChips();
	toast(`Model set to ${modelId}`);
}
async function openModelPicker() {
	if (!state.connected) {
		toast("Connect the local service before choosing a model.");
		return;
	}
	if (!state.capabilities.has("composer.model_picker.v1")) {
		openDrawer("settings-drawer");
		return;
	}
	openContextPicker({
		eyebrow: "Model",
		title: "Choose a model",
		placeholder: "Search model or provider",
		hint: "Availability and policy are decided by the connected server.",
		options: (await api(`/v1/models?${catalogQuery()}`)).map((model) => ({
			id: model.id,
			label: model.display_name,
			detail: `${model.provider} · ${model.source.replaceAll("_", " ")}`,
			meta: [
				model.recommended ? "Recommended" : "",
				model.context_window ? `${Math.round(model.context_window / 1e3)}k context` : "",
				model.supports_reasoning === true ? "Reasoning" : "",
				model.cost_tier || ""
			].filter(Boolean),
			current: model.id === (state.session?.model || $("model").value.trim()),
			disabled: !model.available,
			disabledReason: model.locked_reason,
			select: () => chooseModel(model.id)
		}))
	});
}
async function choosePermissionMode(mode) {
	if (state.session) state.permissionMode = (await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/preferences`, {
		method: "PATCH",
		body: JSON.stringify({
			scope: scope(),
			permission_mode: mode
		})
	})).permission_mode;
	else {
		state.permissionMode = mode;
		sessionStorage.setItem("oc.permission-mode", mode);
	}
	updateContextChips();
	toast("The active Team policy still decides what is allowed.");
}
function applyAssistantAlias(alias) {
	state.assistantAlias = alias.trim() || "Opencoding";
	document.querySelectorAll(".message.assistant").forEach((message) => {
		message.setAttribute("aria-label", `${state.assistantAlias} response`);
		const label = message.querySelector(".message-label");
		if (label) label.textContent = state.assistantAlias;
	});
}
async function loadSessionPreferences(sessionId) {
	const preferences = await api(`/v1/sessions/${encodeURIComponent(sessionId)}/preferences?${catalogQuery()}`);
	if (state.session?.id !== sessionId) return;
	state.permissionMode = preferences.permission_mode;
	applyAssistantAlias(preferences.assistant_alias);
	updateContextChips();
}
async function openPermissionPicker() {
	if (!state.connected) {
		toast("Connect the local service before choosing permissions.");
		return;
	}
	if (!state.capabilities.has("composer.permission_picker.v1")) {
		toast("This server does not support persisted permission profiles.");
		return;
	}
	openContextPicker({
		eyebrow: "Permissions",
		title: "Choose a permission profile",
		placeholder: "Search permission behavior",
		hint: "Profiles can reduce prompts but never bypass Team policy or sandbox limits.",
		options: (await api(`/v1/permission-profiles?${catalogQuery()}`)).map((profile) => ({
			id: profile.mode,
			label: profile.label,
			detail: profile.description,
			meta: [
				profile.file_changes,
				profile.commands,
				profile.network,
				profile.source.replaceAll("_", " ")
			],
			current: profile.mode === state.permissionMode,
			disabled: Boolean(profile.locked_reason),
			disabledReason: profile.locked_reason,
			select: () => choosePermissionMode(profile.mode)
		}))
	});
}
async function loadConfigurationSources() {
	const target = $("configuration-sources");
	if (!state.connected) {
		target.className = "configuration-sources empty";
		target.textContent = "Connect to inspect effective configuration.";
		return;
	}
	target.className = "configuration-sources";
	target.textContent = "Loading effective values…";
	const generation = state.generation;
	try {
		const [models, profiles] = await Promise.all([api(`/v1/models?${catalogQuery()}`), api(`/v1/permission-profiles?${catalogQuery()}`)]);
		if (!isCurrent(generation)) return;
		const modelId = state.session?.model || $("model").value.trim();
		const model = models.find((entry) => entry.id === modelId);
		const profile = profiles.find((entry) => entry.mode === state.permissionMode);
		target.replaceChildren();
		[
			{
				label: "Model",
				value: model?.display_name || modelId || "Not configured",
				source: model?.source || "local default",
				locked: model?.locked_reason || null
			},
			{
				label: "Permission profile",
				value: profile?.label || permissionLabels[state.permissionMode],
				source: profile?.source || "session",
				locked: profile?.locked_reason || null
			},
			{
				label: "Workspace",
				value: state.session?.workspace_uri || $("workspace").value.trim() || "Not configured",
				source: state.session ? "session" : "daemon default",
				locked: null
			}
		].forEach((entry) => {
			const row = document.createElement("article");
			row.className = `configuration-source${entry.locked ? " locked" : ""}`;
			const label = document.createElement("strong");
			label.textContent = `${entry.label} · ${entry.value}`;
			const source = document.createElement("span");
			source.textContent = `Source: ${entry.source.replaceAll("_", " ")}`;
			row.append(label, source);
			if (entry.locked) {
				const reason = document.createElement("small");
				reason.textContent = `Managed and locked: ${entry.locked}`;
				row.append(reason);
			}
			target.append(row);
		});
	} catch (error) {
		if (isCurrent(generation)) {
			target.className = "configuration-sources empty";
			target.textContent = `Configuration unavailable: ${error.message}`;
		}
	}
}
function closeDrawers(restoreFocus = true) {
	["inspector", "settings-drawer"].forEach((id) => {
		$(id).classList.remove("open");
		$(id).setAttribute("aria-hidden", "true");
		$(id).setAttribute("inert", "");
	});
	$("toggle-inspector").setAttribute("aria-expanded", "false");
	document.body.classList.remove("drawer-open");
	if (restoreFocus && drawerReturnFocus?.isConnected) drawerReturnFocus.focus();
	if (restoreFocus) drawerReturnFocus = null;
}
function openDrawer(id) {
	const trigger = document.activeElement;
	closeDrawers(false);
	drawerReturnFocus = trigger instanceof HTMLElement ? trigger : null;
	$(id).classList.add("open");
	$(id).setAttribute("aria-hidden", "false");
	$(id).removeAttribute("inert");
	document.body.classList.add("drawer-open");
	if (id === "inspector") $("toggle-inspector").setAttribute("aria-expanded", "true");
	if (id === "settings-drawer") loadConfigurationSources().catch(() => {});
	window.setTimeout(() => (id === "settings-drawer" ? $("organization") : $(`close-${id}`))?.focus(), 0);
}
function toggleHistory() {
	if (window.matchMedia("(max-width: 760px)").matches) document.body.classList.toggle("mobile-sidebar-open");
	else {
		document.body.classList.toggle("sidebar-collapsed");
		const expanded = !document.body.classList.contains("sidebar-collapsed");
		$("toggle-sidebar").setAttribute("aria-expanded", String(expanded));
		sessionStorage.setItem("oc.sidebar-collapsed", String(!expanded));
	}
}
function formScope() {
	return {
		organization_id: $("organization").value.trim(),
		team_id: $("team").value.trim(),
		actor_id: $("actor").value.trim(),
		goal_id: null,
		task_id: null
	};
}
function scope() {
	return state.connected && state.authenticatedScope ? { ...state.authenticatedScope } : formScope();
}
function saveSettings() {
	updateContextChips();
}
function loadSettings() {
	fields.forEach((id) => sessionStorage.removeItem(`oc.${id}`));
}
function daemonSettings() {
	return {
		organization_id: $("organization").value.trim(),
		team_id: $("team").value.trim(),
		actor_id: $("actor").value.trim(),
		workspace_uri: $("workspace").value.trim(),
		default_model: $("model").value.trim(),
		default_title: $("title").value.trim(),
		telemetry_enabled: false,
		max_context_tokens: 32e3
	};
}
function applyDaemonSettings(settings) {
	$("organization").value = settings.organization_id;
	$("team").value = settings.team_id;
	$("actor").value = settings.actor_id;
	$("workspace").value = settings.workspace_uri;
	$("model").value = settings.default_model;
	$("title").value = settings.default_title;
	updateContextChips();
}
function protocolMajor(version) {
	const value = Number.parseInt(String(version).split(".")[0], 10);
	return Number.isSafeInteger(value) && value >= 0 ? value : null;
}
function negotiateCapabilities(manifest) {
	if (!manifest || protocolMajor(manifest.protocol_version) !== 1) throw new Error(`Incompatible daemon protocol ${manifest?.protocol_version || "unknown"}`);
	const enabled = new Set((Array.isArray(manifest.capabilities) ? manifest.capabilities : []).filter((item) => item?.enabled && protocolMajor(item.version) === 1).map((item) => String(item.id)));
	for (const required of [
		"scope.team",
		"session.persistence",
		"event.sse_replay"
	]) if (!enabled.has(required)) throw new Error(`Missing required capability ${required} v1`);
	return enabled;
}
async function api(path, options = {}) {
	const { allowDisconnected = false } = options;
	if (!allowDisconnected && !state.connected) throw new Error("daemon is not connected");
	return requestJson(path, options);
}
async function bootstrapBrowserSession() {
	const meta = document.querySelector("meta[name=\"opencoding-bootstrap\"]");
	const token = meta?.content || "";
	meta?.remove();
	if (!token || token === "__OPENCODING_BOOTSTRAP__") return false;
	const response = await fetch("/v1/auth/bootstrap", {
		method: "POST",
		cache: "no-store",
		credentials: "same-origin",
		referrerPolicy: "no-referrer",
		headers: {
			"content-type": "application/json",
			"x-opencoding-csrf": "1"
		},
		body: JSON.stringify({ token })
	});
	if (!response.ok) throw new Error(`automatic daemon authentication failed (${response.status})`);
	return true;
}
function connectionRecovery(label) {
	if (label.startsWith("Incompatible daemon protocol")) return {
		title: "Opencoding update required.",
		description: `This Web client supports protocol v1, but the local service reported ${label.replace("Incompatible daemon protocol ", "v")}. Update or reinstall Opencoding so both components use the same version.`,
		action: "Check again"
	};
	if (label.startsWith("Missing required capability")) return {
		title: "Web and local service versions do not match.",
		description: `${label}. Update or reinstall Opencoding, restart the local service, then check again.`,
		action: "Check again"
	};
	if (label === "reconnecting") return {
		title: "Connection interrupted.",
		description: "Your draft is safe. Opencoding is retrying automatically; reconnect now if the local service has restarted.",
		action: "Reconnect now"
	};
	return {
		title: "Local service is unavailable.",
		description: "Your draft is safe. Restart the Opencoding local service, then retry or open Diagnostics.",
		action: "Retry"
	};
}
function renderClientPresence() {
	const menu = $("presence-menu");
	menu.hidden = !state.connected || !state.capabilities.has("client.presence.v1") || clientPresence.length === 0;
	if (menu.hidden) return;
	const count = clientPresence.filter((client) => client.session_id === state.session?.id).length || clientPresence.length;
	$("presence-summary").textContent = `${count} client${count === 1 ? "" : "s"}`;
	const list = $("presence-list");
	list.replaceChildren();
	clientPresence.forEach((client) => {
		const row = document.createElement("article");
		row.className = "presence-client";
		row.setAttribute("role", "listitem");
		const title = document.createElement("strong");
		const own = client.client_id === presenceClientId;
		title.textContent = `${own ? "You" : client.actor_id} · ${client.client_kind.toUpperCase()}${client.remote ? " · remote" : ""}`;
		const details = document.createElement("span");
		const session = state.sessions.find((candidate) => candidate.id === client.session_id);
		details.textContent = [
			client.focused ? "active" : "background",
			session?.title || (client.session_id ? `session ${client.session_id}` : "no session"),
			client.device_id ? `device ${client.device_id}` : null
		].filter(Boolean).join(" · ");
		row.append(title, details);
		if (client.remote && client.revocable) {
			const revoke = document.createElement("button");
			revoke.type = "button";
			revoke.textContent = own ? "Disconnect" : "Revoke";
			revoke.addEventListener("click", () => {
				revokeRemoteClient(client).catch((error) => toast(error.message));
			});
			row.append(revoke);
		}
		list.append(row);
	});
}
async function updateClientPresence() {
	if (!state.connected || !state.capabilities.has("client.presence.v1")) return;
	clientPresence = await api("/v1/client-presence", {
		method: "PUT",
		body: JSON.stringify({
			scope: scope(),
			client_id: presenceClientId,
			client_kind: "web",
			session_id: state.session?.id || null,
			focused: document.visibilityState === "visible" && document.hasFocus()
		})
	});
	renderClientPresence();
}
async function revokeRemoteClient(client) {
	const own = client.client_id === presenceClientId;
	if (!await requestAction({
		eyebrow: "Remote client",
		title: `${own ? "Disconnect" : "Revoke"} ${client.client_kind.toUpperCase()} client?`,
		description: "The short-lived Team Grant will be rejected immediately and after daemon restart. The registered device must obtain a new grant under Enterprise policy.",
		details: [
			`Actor: ${client.actor_id}`,
			`Device: ${client.device_id || "unknown"}`,
			`Client: ${client.client_id}`,
			`Presence expires: ${new Date(client.expires_at).toLocaleString()}`
		],
		confirm: own ? "Disconnect client" : "Revoke grant",
		danger: true
	})) return;
	clientPresence = await api(`/v1/client-presence/${encodeURIComponent(client.client_id)}`, {
		method: "DELETE",
		body: JSON.stringify({
			scope: scope(),
			revoke_remote_grant: true
		})
	});
	renderClientPresence();
	if (own) setConnection(false, "Remote client grant revoked");
	else toast("Remote client grant revoked");
}
function setConnection(ok, label = ok ? "connected" : "offline") {
	if (!ok) state.generation += 1;
	state.connecting = false;
	state.connected = ok;
	$("connection").replaceChildren();
	const dot = document.createElement("span");
	$("connection").append(dot, document.createTextNode(label));
	$("connection").className = `status-dot ${ok ? "online" : "offline"}`;
	const arrow = document.createElement("span");
	arrow.setAttribute("aria-hidden", "true");
	arrow.textContent = "→";
	$("empty-connect").replaceChildren(document.createTextNode(ok ? "Change workspace " : "Connect workspace "), arrow);
	$("empty-guidance").textContent = ok ? "Describe the outcome. Opencoding plans, edits, tests, and shows every change before you merge." : "Connect a workspace, then describe the outcome. Opencoding plans, edits, tests, and shows every change.";
	const recovery = connectionRecovery(label);
	$("recovery-title").textContent = recovery.title;
	$("recovery-description").textContent = recovery.description;
	$("retry-connection").textContent = recovery.action;
	$("offline-recovery").hidden = ok || label === "connecting";
	announce(ok ? "Daemon connected" : `Daemon ${label}`);
	if (!ok) {
		state.authenticatedScope = null;
		state.capabilities = /* @__PURE__ */ new Set();
		state.sessions = [];
		clientPresence = [];
		$("presence-menu").hidden = true;
		clearSessionSelection(false, false);
		$("sessions").replaceChildren(document.createTextNode("Connect to load history"));
		$("sessions").className = "sessions empty";
		$("team-metrics").replaceChildren();
		[
			["team-goals", "No active goals"],
			["team-resources", "No owned resources"],
			["team-queue", "No queued work"],
			["durable-task-list", "No background tasks"],
			["background-terminal-list", "No background terminals"],
			["agent-run-list", "No agent runs"],
			["team-budget-list", "No budgets configured"],
			["team-approval-list", "No approval decisions"],
			["team-outcome-list", "No verified outcomes"],
			["outcome-summary", "No verified outcomes"],
			["team-audit-list", "No audit activity"]
		].forEach(([id, text]) => {
			$(id).replaceChildren(document.createTextNode(text));
			$(id).className = "team-queue empty";
		});
		$("capacity-summary").replaceChildren();
		$("activity").replaceChildren(document.createTextNode("No activity yet"));
		$("activity").className = "activity empty";
		setToolMessage("No result selected.");
	}
	updateContextChips();
}
async function connect() {
	saveSettings();
	if (state.reconnectTimer) {
		clearTimeout(state.reconnectTimer);
		state.reconnectTimer = null;
	}
	if (state.abort) state.abort.abort();
	setConnection(false, "connecting");
	state.connecting = true;
	state.abort = new AbortController();
	const signal = state.abort.signal;
	state.after = 0;
	const generation = state.generation;
	try {
		const capabilities = negotiateCapabilities(await api("/v1/capabilities", {
			allowDisconnected: true,
			signal
		}));
		if (generation !== state.generation) return;
		state.capabilities = capabilities;
		if (settingsDirty) await api("/v1/settings", {
			method: "PUT",
			body: JSON.stringify(daemonSettings()),
			allowDisconnected: true,
			signal
		});
		else {
			applyDaemonSettings(await api("/v1/settings", {
				allowDisconnected: true,
				signal
			}));
			saveSettings();
		}
		if (generation !== state.generation) return;
		settingsDirty = false;
		state.authenticatedScope = formScope();
		setConnection(true);
		await Promise.all([refreshSessions(), refreshTeam()]);
		await restoreRoute();
		await updateClientPresence();
		subscribe(generation);
		closeDrawers();
		$("prompt").focus();
		toast("Workspace connected");
	} catch (error) {
		if (generation === state.generation) setConnection(false, error.message);
	}
}
async function refreshSessions() {
	const generation = state.generation;
	const s = scope();
	const sessions = await api(`/v1/sessions?${new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	})}`);
	if (!state.connected || generation !== state.generation) return;
	state.sessions = sessions;
	renderSessions();
	if (!$("projects-view").hidden) {
		const route = parseRoute(window.location.pathname);
		renderProjects(route.type === "project" ? route.projectId : null).catch(() => {});
	}
	if (!$("artifacts-view").hidden) renderArtifactList();
}
function renderSessions() {
	const container = $("sessions");
	container.replaceChildren();
	container.className = "sessions";
	const query = $("session-search").value.trim().toLowerCase();
	const status = $("session-filter").value;
	const sessions = state.sessions.filter((session) => {
		if (status !== "all" && session.status !== status) return false;
		return !query || [
			session.title,
			workspaceName(session.workspace_uri),
			session.model
		].some((value) => String(value || "").toLowerCase().includes(query));
	});
	if (!sessions.length) {
		container.textContent = state.sessions.length ? "No matching tasks" : "No sessions yet";
		container.classList.add("empty");
		return;
	}
	sessionWindowStart = Math.min(sessionWindowStart, Math.max(0, sessions.length - SESSION_WINDOW_SIZE));
	const visibleSessions = sessions.slice(sessionWindowStart, sessionWindowStart + SESSION_WINDOW_SIZE);
	const sessionWindowControl = (label, nextStart) => {
		const button = document.createElement("button");
		button.type = "button";
		button.className = "session-window-control";
		button.textContent = label;
		button.addEventListener("click", () => {
			sessionWindowStart = nextStart;
			renderSessions();
			container.querySelector(".session, .session-window-control")?.focus();
		});
		return button;
	};
	if (sessionWindowStart > 0) container.append(sessionWindowControl(`Show previous ${Math.min(SESSION_WINDOW_SIZE, sessionWindowStart)} sessions`, Math.max(0, sessionWindowStart - SESSION_WINDOW_SIZE)));
	visibleSessions.forEach((session) => {
		const button = document.createElement("button");
		button.className = `session${state.session?.id === session.id ? " active" : ""}`;
		const title = document.createElement("span");
		title.textContent = session.title;
		const detail = document.createElement("small");
		detail.textContent = `${workspaceName(session.workspace_uri)} · ${session.model}`;
		button.append(title, detail);
		button.addEventListener("click", () => selectSession(session));
		container.append(button);
	});
	const remainingSessions = sessions.length - sessionWindowStart - visibleSessions.length;
	if (remainingSessions > 0) container.append(sessionWindowControl(`Show next ${Math.min(SESSION_WINDOW_SIZE, remainingSessions)} sessions`, sessionWindowStart + SESSION_WINDOW_SIZE));
}
async function createSession() {
	const generation = state.generation;
	try {
		saveSettings();
		if (!$("workspace").value.trim() || !$("model").value.trim()) throw new Error("Workspace and model are required");
		const session = await api("/v1/sessions", {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				workspace_uri: $("workspace").value.trim(),
				title: $("title").value.trim(),
				model: $("model").value.trim()
			})
		});
		if (!isCurrent(generation)) return null;
		transferNewComposerDraft(session);
		if (state.permissionMode !== "manual") await api(`/v1/sessions/${encodeURIComponent(session.id)}/preferences`, {
			method: "PATCH",
			body: JSON.stringify({
				scope: scope(),
				permission_mode: state.permissionMode
			})
		});
		await refreshSessions();
		await selectSession(session);
		closeDrawers();
		return session;
	} catch (error) {
		if (isCurrent(generation)) {
			addActivity("session.error", { error: error.message });
			openDrawer("settings-drawer");
		}
		return null;
	}
}
async function selectSession(session, { updateRoute = true } = {}) {
	if (!state.connected) return;
	saveComposerDraft();
	closeMentionMenu();
	transcriptFollowing = true;
	$("jump-latest").hidden = true;
	transcriptProjection = selectTranscriptSession(transcriptProjection, session.id);
	loadedTranscriptSnapshot = null;
	$("load-earlier").hidden = true;
	state.pendingInputs = [];
	renderPendingInputs();
	state.session = session;
	state.turn = null;
	setTurnRunning(false);
	$("undo-turn").disabled = true;
	$("show-context").disabled = !state.capabilities.has("context.explain");
	$("review-session").disabled = !state.capabilities.has("review.read_only");
	$("show-checkpoints").disabled = false;
	$("fork-session").disabled = false;
	$("show-branches").disabled = false;
	$("export-session").disabled = false;
	$("rename-session").disabled = false;
	$("rename-assistant").disabled = false;
	$("cancel-session").disabled = !["active", "archived"].includes(session.status);
	$("cancel-session").textContent = session.status === "archived" ? "Restore" : "Archive";
	$("cancel-session").classList.toggle("danger", session.status !== "archived");
	$("delete-session").disabled = false;
	$("quick-diff").disabled = false;
	$("session-title").textContent = session.title;
	$("session-meta").textContent = `${session.status} · ${session.model} · ${session.workspace_uri}`;
	restoreComposerDraft();
	document.body.classList.remove("mobile-sidebar-open");
	showWorkspace({ updateRoute });
	const generation = state.generation;
	const sessionId = session.id;
	const query = catalogQuery();
	const preferencesRequest = state.capabilities.has("composer.permission_picker.v1") ? api(`/v1/sessions/${encodeURIComponent(session.id)}/preferences?${query}`) : Promise.resolve({
		session_id: session.id,
		permission_mode: "manual",
		assistant_alias: "Opencoding",
		source: "compatibility_default",
		locked_reason: null,
		updated_at: session.updated_at || (/* @__PURE__ */ new Date(0)).toISOString()
	});
	const sideConversationsRequest = state.capabilities.has("session.side_conversation.v1") ? api(`/v1/side-conversations?${query}`).catch(() => []) : Promise.resolve([]);
	const goalRequest = state.capabilities.has("session.goal.v1") ? api(`/v1/sessions/${encodeURIComponent(session.id)}/goal?${query}`) : Promise.resolve(null);
	const [, , preferences, sideConversations, goal] = await Promise.all([
		refreshSessions(),
		loadMessages(),
		preferencesRequest,
		sideConversationsRequest,
		goalRequest
	]);
	if (isCurrent(generation) && state.session?.id === sessionId) {
		state.permissionMode = preferences.permission_mode;
		applyAssistantAlias(preferences.assistant_alias);
		state.goal = goal;
		state.sideConversation = sideConversations.find((conversation) => conversation.session_id === sessionId && conversation.status === "active") || null;
		updateContextChips();
		renderSessionGoal();
		renderSideConversationState();
	}
	updateClientPresence().catch((error) => addActivity("client.presence.error", { error: error.message }));
	$("prompt").focus();
}
function clearSessionSelection(refresh = true, updateRoute = true) {
	saveComposerDraft();
	closeMentionMenu();
	transcriptFollowing = true;
	$("jump-latest").hidden = true;
	transcriptProjection = selectTranscriptSession(transcriptProjection, null);
	loadedTranscriptSnapshot = null;
	$("load-earlier").hidden = true;
	state.session = null;
	state.goal = null;
	renderSessionGoal();
	state.sideConversation = null;
	renderSideConversationState();
	state.turn = null;
	state.pendingInputs = [];
	renderPendingInputs();
	setTurnRunning(false);
	state.approvals.clear();
	state.questions.clear();
	applyAssistantAlias("Opencoding");
	$("session-title").textContent = "New task";
	$("session-meta").textContent = "Ready when you are";
	$("messages").replaceChildren();
	$("approvals").replaceChildren();
	$("rename-session").disabled = true;
	$("rename-assistant").disabled = true;
	$("show-context").disabled = true;
	$("review-session").disabled = true;
	$("show-checkpoints").disabled = true;
	$("fork-session").disabled = true;
	$("show-branches").disabled = true;
	$("export-session").disabled = true;
	$("cancel-session").disabled = true;
	$("cancel-session").textContent = "Archive";
	$("cancel-session").classList.add("danger");
	$("delete-session").disabled = true;
	$("undo-turn").disabled = true;
	$("quick-diff").disabled = true;
	$("turn-state").textContent = "idle";
	updateConversationState(false);
	if (refresh && state.connected) refreshSessions().catch(() => {});
	$("prompt").focus();
	state.toolSteps.clear();
	state.itemsById.clear();
	const savedPermission = sessionStorage.getItem("oc.permission-mode");
	state.permissionMode = savedPermission === "accept_edits" || savedPermission === "plan" ? savedPermission : "manual";
	updateContextChips();
	restoreComposerDraft();
	showWorkspace({ updateRoute });
	updateClientPresence().catch(() => {});
}
function setTurnRunning(running) {
	state.turnRunning = running;
	updateSendAction();
	announce(running ? "Task running. Type another message to queue it, or use the stop button with an empty prompt." : "Task stopped.");
}
function updateSendAction() {
	const hasAttachments = state.draftFiles.length > 0;
	const hasInput = Boolean($("prompt").value.trim()) || hasAttachments;
	const queues = state.turnRunning && hasInput;
	$("send-turn").classList.toggle("is-stop", state.turnRunning && !queues);
	$("send-turn").textContent = queues ? "＋" : state.turnRunning ? "■" : "↑";
	$("send-turn").disabled = composerSubmissionPending || state.turnRunning && hasAttachments;
	$("send-turn").setAttribute("aria-label", state.turnRunning && hasAttachments ? "Remove attachments before queuing" : queues ? "Queue message" : state.turnRunning ? "Stop turn" : "Run turn");
	$("steer-turn").hidden = !queues || hasAttachments || !state.capabilities.has("turn.input_queue.v1");
}
async function withComposerSubmission(action) {
	if (composerSubmissionPending) return;
	composerSubmissionPending = true;
	updateSendAction();
	try {
		await action();
	} finally {
		composerSubmissionPending = false;
		updateSendAction();
	}
}
function renderPendingInputs() {
	const target = $("pending-inputs");
	target.replaceChildren();
	target.hidden = state.pendingInputs.length === 0;
	state.pendingInputs.forEach((item, index) => {
		const row = document.createElement("div");
		row.className = "pending-input";
		const label = document.createElement("span");
		label.textContent = `${item.mode === "steer" ? "Steering" : index === 0 ? "Next" : `Queued ${index + 1}`} · ${typeof item.content === "string" ? item.content : "Structured input"}`;
		const remove = document.createElement("button");
		remove.type = "button";
		remove.setAttribute("aria-label", `Remove queued message ${index + 1}`);
		remove.textContent = "×";
		remove.addEventListener("click", async () => {
			remove.disabled = true;
			try {
				await api(`/v1/turn-inputs/${encodeURIComponent(item.id)}`, {
					method: "DELETE",
					body: JSON.stringify({ scope: scope() })
				});
				state.pendingInputs = state.pendingInputs.filter((candidate) => candidate.id !== item.id);
				renderPendingInputs();
			} catch (error) {
				remove.disabled = false;
				addActivity("turn.input.cancel.error", { error: error.message });
			}
		});
		row.append(label, remove);
		target.append(row);
	});
}
async function refreshPendingInputs() {
	if (!state.session) return;
	const selected = state.session.id;
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	});
	const inputs = await api(`/v1/sessions/${encodeURIComponent(selected)}/inputs?${query}`);
	if (state.session?.id !== selected) return;
	state.pendingInputs = inputs;
	renderPendingInputs();
}
async function submitTurnInput(content, mode) {
	if (!state.session || !state.turn) throw new Error("No running turn is available");
	const input = await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/inputs`, {
		method: "POST",
		body: JSON.stringify({
			scope: scope(),
			target_turn_id: state.turn,
			mode,
			content,
			idempotency_key: `web-${globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(16).slice(2)}`}`
		})
	});
	if (!state.pendingInputs.some((candidate) => candidate.id === input.id)) state.pendingInputs.push(input);
	renderPendingInputs();
	$("prompt").value = "";
	sessionStorage.removeItem(composerTextDraftKey());
	resizePrompt();
	updateSendAction();
	toast(mode === "steer" ? "Steering the current turn" : "Message durably queued");
	return input;
}
async function cancelActiveTurn() {
	if (!state.turn) return;
	const generation = state.generation;
	$("turn-state").textContent = "cancelling";
	try {
		await api(`/v1/turns/${encodeURIComponent(state.turn)}/cancel`, {
			method: "POST",
			body: JSON.stringify({ scope: scope() })
		});
	} catch (error) {
		if (isCurrent(generation)) {
			setTurnRunning(false);
			addActivity("turn.cancel.error", { error: error.message });
		}
	}
}
async function closeSession(deleting) {
	if (!state.session) return;
	if (!deleting && state.session.status === "archived") {
		const generation = state.generation;
		try {
			const restored = await api(`/v1/sessions/${encodeURIComponent(state.session.id)}`, {
				method: "PATCH",
				body: JSON.stringify({
					scope: scope(),
					status: "active"
				})
			});
			if (!isCurrent(generation)) return;
			await selectSession(restored);
			toast("Session restored");
		} catch (error) {
			addActivity("session.lifecycle.error", { error: error.message });
		}
		return;
	}
	const generation = state.generation;
	let impact = null;
	if (deleting) {
		const s = scope();
		const query = new URLSearchParams({
			organization_id: s.organization_id,
			team_id: s.team_id,
			actor_id: s.actor_id
		});
		try {
			impact = await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/impact?${query}`);
		} catch (error) {
			if (isCurrent(generation)) setToolMessage(`Unable to preview delete impact: ${error.message}`, true);
			return;
		}
	}
	if (!await requestAction({
		eyebrow: "Session",
		title: deleting ? "Delete this session?" : "Archive this session?",
		description: deleting ? "Review the exact retained and affected records before deleting." : "Active turns stop and the session remains available in history.",
		details: impact ? [
			`${impact.turn_count} turns · ${impact.message_count} messages`,
			`${impact.attachment_count} attachments · ${impact.artifact_count} artifacts`,
			`${impact.branch_count} linked branches remain independent`,
			impact.active_turn_count ? `${impact.active_turn_count} active turns will stop` : "No active turns will be stopped",
			impact.audit_evidence_preserved ? "Audit evidence is preserved" : "Audit evidence is removed"
		] : [],
		confirm: deleting ? "Delete session" : "Archive session",
		danger: true
	})) return;
	const id = encodeURIComponent(state.session.id);
	const path = deleting ? `/v1/sessions/${id}` : `/v1/sessions/${id}/cancel`;
	try {
		const closed = await api(path, {
			method: deleting ? "DELETE" : "POST",
			body: JSON.stringify({ scope: scope() })
		});
		if (!isCurrent(generation)) return;
		addActivity(deleting ? "session.deleted" : "session.cancelled", { session_id: closed.id });
		clearSessionSelection();
		await refreshSessions();
		toast(deleting ? "Session deleted" : "Session archived");
	} catch (error) {
		addActivity("session.lifecycle.error", { error: error.message });
	}
}
async function renameSession() {
	if (!state.session) return;
	const generation = state.generation;
	const values = await requestAction({
		eyebrow: "Session",
		title: "Rename session",
		description: "Use a short title that makes this task easy to find.",
		confirm: "Rename",
		fields: [{
			name: "title",
			label: "Session title",
			value: state.session.title,
			required: true,
			maxlength: 64
		}]
	});
	if (!values?.title) return;
	try {
		const updated = await api(`/v1/sessions/${encodeURIComponent(state.session.id)}`, {
			method: "PATCH",
			body: JSON.stringify({
				scope: scope(),
				title: values.title
			})
		});
		if (!isCurrent(generation)) return;
		state.session = updated;
		$("session-title").textContent = updated.title;
		$("session-meta").textContent = `${updated.status} · ${updated.model} · ${updated.workspace_uri}`;
		await refreshSessions();
		toast("Session renamed");
	} catch (error) {
		addActivity("session.lifecycle.error", { error: error.message });
	}
}
async function renameAssistant() {
	if (!state.session) return;
	const sessionId = state.session.id;
	const values = await requestAction({
		eyebrow: "Session appearance",
		title: "Rename the assistant",
		description: "This changes the assistant heading in this Session. It does not rename the Session or rewrite history.",
		confirm: "Save name",
		fields: [{
			name: "alias",
			label: "Assistant name",
			value: state.assistantAlias,
			required: true,
			maxlength: 40
		}]
	});
	if (!values?.alias) return;
	const preferences = await api(`/v1/sessions/${encodeURIComponent(sessionId)}/preferences`, {
		method: "PATCH",
		body: JSON.stringify({
			scope: scope(),
			assistant_alias: values.alias.trim()
		})
	});
	if (state.session?.id !== sessionId) return;
	applyAssistantAlias(preferences.assistant_alias);
	toast(`Assistant name set to ${preferences.assistant_alias}`);
}
async function forkSession() {
	if (!state.session) return;
	const generation = state.generation;
	const source = state.session;
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	});
	let turns;
	try {
		turns = (await api(`/v1/sessions/${encodeURIComponent(source.id)}/turns?${query}`)).filter((turn) => [
			"completed",
			"failed",
			"cancelled"
		].includes(turn.status));
	} catch (error) {
		toast(error.message);
		return;
	}
	if (!isCurrent(generation) || state.session?.id !== source.id) return;
	const fields = [{
		name: "title",
		label: "Fork title",
		value: `Fork of ${source.title}`.slice(0, 64),
		required: true,
		maxlength: 64
	}];
	if (turns.length) {
		const latestTurn = turns.at(-1);
		if (!latestTurn) return;
		fields.push({
			name: "source_turn_id",
			label: "Branch after",
			value: latestTurn.id,
			options: turns.map((turn, index) => [turn.id, `${index + 1}. ${turn.status} · ${new Date(turn.started_at).toLocaleString()}`])
		});
	}
	const values = await requestAction({
		eyebrow: "Session",
		title: "Fork conversation",
		description: "Create an independent branch from a completed checkpoint. Future turns will not affect the source.",
		confirm: "Create fork",
		fields
	});
	if (!values?.title) return;
	try {
		const fork = await api(`/v1/sessions/${encodeURIComponent(source.id)}/fork`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				title: values.title,
				source_turn_id: values.source_turn_id || null
			})
		});
		if (!isCurrent(generation)) return;
		await refreshSessions();
		await selectSession(fork);
		toast("Conversation forked");
	} catch (error) {
		addActivity("session.fork.error", { error: error.message });
	}
}
function renderSideConversationState() {
	const active = state.sideConversation?.status === "active" ? state.sideConversation : null;
	$("side-conversation-banner").hidden = !active;
	if (active) $("side-conversation-banner").querySelector("span").textContent = `Temporary Fork of ${active.source_session_id}. Promote it to keep it as a regular Session.`;
}
async function createSideConversation() {
	if (!state.session || state.turnRunning) return;
	const source = state.session;
	const values = await requestAction({
		eyebrow: "Side conversation",
		title: "Ask without changing this Session",
		description: "Creates a temporary Fork at the latest completed Turn. The answer stays separate until you promote it.",
		confirm: "Start side conversation",
		fields: [{
			name: "prompt",
			label: "Question",
			multiline: true,
			maxlength: 8e3,
			placeholder: "Explain whether this alternative is safer without changing any files.",
			required: true
		}]
	});
	if (!values?.prompt) return;
	const result = await api(`/v1/sessions/${encodeURIComponent(source.id)}/side-conversations`, {
		method: "POST",
		body: JSON.stringify({
			scope: source.scope,
			source_turn_id: null,
			prompt: values.prompt.trim()
		})
	});
	await refreshSessions();
	await selectSession(result.session);
	state.sideConversation = result.conversation;
	renderSideConversationState();
	toast("Side conversation started");
}
async function promoteSideConversation() {
	const side = state.sideConversation;
	if (!side || side.status !== "active") return;
	if (!await requestAction({
		eyebrow: "Side conversation",
		title: "Promote to a regular Session?",
		description: "The Fork becomes normal project history and is no longer treated as temporary.",
		confirm: "Promote Session"
	})) return;
	await api(`/v1/side-conversations/${encodeURIComponent(side.id)}/promote`, {
		method: "POST",
		body: JSON.stringify({ scope: side.scope })
	});
	state.sideConversation = null;
	renderSideConversationState();
	await refreshSessions();
	toast("Side conversation promoted");
}
async function closeSideConversation() {
	const side = state.sideConversation;
	if (!side || side.status !== "active") return;
	const source = state.sessions.find((session) => session.id === side.source_session_id);
	if (!await requestAction({
		eyebrow: "Side conversation",
		title: "Close this temporary Fork?",
		description: "The temporary Session is archived. The source Session and its Transcript are unchanged.",
		confirm: "Close side conversation",
		danger: true
	})) return;
	await api(`/v1/side-conversations/${encodeURIComponent(side.id)}`, {
		method: "DELETE",
		body: JSON.stringify({ scope: side.scope })
	});
	state.sideConversation = null;
	renderSideConversationState();
	await refreshSessions();
	if (source) await selectSession(source);
	else clearSessionSelection();
	toast("Side conversation closed");
}
function renderMessageAttachments(item, attachments) {
	item.querySelector(".message-attachments")?.remove();
	if (!attachments.length) return;
	const list = document.createElement("div");
	list.className = "message-attachments";
	list.setAttribute("aria-label", "Message attachments");
	attachments.forEach((attachment) => {
		const chip = document.createElement("span");
		chip.className = "message-attachment";
		const name = document.createElement("strong");
		name.textContent = attachment.file_name;
		const detail = document.createElement("span");
		detail.textContent = `${attachment.media_type} · ${formatBytes(Number(attachment.byte_length))}`;
		chip.append(name, detail);
		list.append(chip);
	});
	item.append(list);
}
function renderMessage(role, content, identity = {}, attachments = []) {
	const item = document.createElement("article");
	item.className = `message ${role}`;
	if (identity.itemId) item.dataset.itemId = identity.itemId;
	if (identity.turnId) item.dataset.turnId = identity.turnId;
	item.setAttribute("aria-label", role === "assistant" ? `${state.assistantAlias} response` : "Your message");
	if (role === "assistant") {
		const label = document.createElement("div");
		label.className = "message-label";
		label.textContent = state.assistantAlias;
		item.append(label);
	}
	renderMessageContent(item, content);
	renderMessageAttachments(item, attachments);
	if (role === "user" && identity.turnId && typeof content === "string") {
		const turnId = identity.turnId;
		const actions = document.createElement("div");
		actions.className = "message-actions";
		const retry = document.createElement("button");
		retry.type = "button";
		retry.textContent = "Edit and retry";
		retry.setAttribute("aria-label", "Edit this message and retry from here");
		retry.addEventListener("click", () => editAndRetry(turnId, content));
		actions.append(retry);
		item.append(actions);
	}
	$("messages").append(item);
	if (identity.itemId) state.itemsById.set(identity.itemId, item);
	updateConversationState(true);
	return item;
}
async function editAndRetry(turnId, originalContent) {
	if (state.turnRunning) {
		toast("Stop the running Turn before retrying an earlier message.");
		return;
	}
	const values = await requestAction({
		eyebrow: "Branch and retry",
		title: "Edit message and retry?",
		description: "Opencoding creates a new branch before this Turn. The original Session and its evidence remain unchanged.",
		confirm: "Retry in new branch",
		fields: [{
			name: "content",
			label: "Message",
			value: originalContent,
			multiline: true,
			required: true,
			maxlength: 2e5
		}]
	});
	if (!values?.content.trim()) return;
	const generation = state.generation;
	try {
		const result = await api(`/v1/turns/${encodeURIComponent(turnId)}/retry`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				content: values.content.trim()
			})
		});
		if (!isCurrent(generation)) return;
		await selectSession(result.session);
		if (!isCurrent(generation)) return;
		state.turn = result.turn.id;
		setTurnRunning(true);
		toast("Retry started in a new branch");
	} catch (error) {
		if (isCurrent(generation)) addActivity("turn.retry.error", { error: error.message });
	}
}
function renderPlan(itemId, turnId, title, steps = [], status = "streaming") {
	let item = state.itemsById.get(itemId);
	if (!item?.classList.contains("plan-card")) {
		item = document.createElement("article");
		item.className = "plan-card";
		item.dataset.itemId = itemId;
		if (turnId) item.dataset.turnId = turnId;
		item.setAttribute("aria-label", "Execution plan");
		$("messages").append(item);
		state.itemsById.set(itemId, item);
	}
	item.classList.toggle("complete", status === "completed" || steps.length > 0 && steps.every((step) => step.status === "completed"));
	item.replaceChildren();
	const heading = document.createElement("div");
	heading.className = "plan-heading";
	const label = document.createElement("strong");
	label.textContent = title || "Plan";
	const count = document.createElement("small");
	count.textContent = `${steps.filter((step) => step.status === "completed").length}/${steps.length} complete`;
	heading.append(label, count);
	const list = document.createElement("ol");
	steps.forEach((step) => {
		const row = document.createElement("li");
		row.className = `plan-step ${step.status}`;
		const marker = document.createElement("span");
		marker.setAttribute("aria-hidden", "true");
		marker.textContent = step.status === "completed" ? "✓" : step.status === "in_progress" ? "●" : "○";
		const text = document.createElement("span");
		text.textContent = step.text;
		row.append(marker, text);
		list.append(row);
	});
	item.append(heading, list);
	updateConversationState(true);
	return item;
}
function renderTranscriptNotice(itemId, turnId, kind, title, detail) {
	let item = state.itemsById.get(itemId);
	if (!item?.classList.contains("transcript-notice")) {
		item = document.createElement("article");
		item.className = "transcript-notice";
		item.dataset.itemId = itemId;
		if (turnId) item.dataset.turnId = turnId;
		$("messages").append(item);
		state.itemsById.set(itemId, item);
	}
	item.className = `transcript-notice ${kind.replaceAll("_", "-")}`;
	item.replaceChildren();
	const label = document.createElement("strong");
	label.textContent = title;
	const copy = document.createElement("span");
	copy.textContent = detail;
	item.append(label, copy);
	if (kind === "warning") item.setAttribute("role", "alert");
	else item.setAttribute("role", "status");
	updateConversationState(true);
	return item;
}
function renderArtifact(itemId, turnId, artifactId, title, mediaType) {
	let item = state.itemsById.get(itemId);
	if (!item?.classList.contains("artifact-card")) {
		item = document.createElement("article");
		item.className = "artifact-card";
		item.dataset.itemId = itemId;
		if (turnId) item.dataset.turnId = turnId;
		$("messages").append(item);
		state.itemsById.set(itemId, item);
	}
	item.replaceChildren();
	const copy = document.createElement("div");
	const label = document.createElement("strong");
	label.textContent = title || "Artifact";
	const type = document.createElement("small");
	type.textContent = mediaType || "content";
	copy.append(label, type);
	const open = document.createElement("button");
	open.type = "button";
	open.textContent = "Open";
	open.setAttribute("aria-label", `Open artifact ${title || artifactId}`);
	open.addEventListener("click", () => openArtifact(artifactId));
	item.append(copy, open);
	updateConversationState(true);
	return item;
}
function renderReviewReport(target, value) {
	if (!isJsonObject(value) || !Array.isArray(value.findings) || typeof value.target !== "string") throw new Error("Review report has an invalid shape");
	const report = value;
	const summary = document.createElement("div");
	summary.className = "review-summary";
	const title = document.createElement("strong");
	title.textContent = `${report.findings.length} actionable finding${report.findings.length === 1 ? "" : "s"}`;
	const scope = document.createElement("small");
	scope.textContent = report.target;
	summary.append(title, scope);
	target.append(summary);
	if (!report.findings.length) {
		const empty = document.createElement("p");
		empty.className = "review-empty";
		empty.textContent = "No actionable findings were reported.";
		target.append(empty);
		return;
	}
	report.findings.forEach((finding) => {
		const card = document.createElement("article");
		card.className = `review-finding severity-${finding.severity}`;
		card.dataset.findingId = finding.id;
		const heading = document.createElement("div");
		heading.className = "review-finding-heading";
		const identity = document.createElement("div");
		const severity = document.createElement("span");
		severity.className = "review-severity";
		severity.textContent = finding.severity;
		const findingTitle = document.createElement("strong");
		findingTitle.textContent = finding.title;
		identity.append(severity, findingTitle);
		const location = `${finding.location.path}:${finding.location.line_start}${finding.location.line_end === finding.location.line_start ? "" : `-${finding.location.line_end}`}`;
		const locate = document.createElement("button");
		locate.type = "button";
		locate.className = "review-location";
		locate.textContent = location;
		locate.setAttribute("aria-label", `Copy review location ${location}`);
		locate.addEventListener("click", () => copyText(location, "Review location copied"));
		heading.append(identity, locate);
		const description = document.createElement("p");
		description.textContent = finding.description;
		const evidence = document.createElement("p");
		evidence.className = "review-evidence";
		const evidenceLabel = document.createElement("strong");
		evidenceLabel.textContent = "Evidence";
		evidence.append(evidenceLabel, document.createTextNode(` · ${finding.evidence}`));
		card.append(heading, description, evidence);
		if (finding.suggested_fix) {
			const fix = document.createElement("p");
			fix.className = "review-fix";
			const fixLabel = document.createElement("strong");
			fixLabel.textContent = "Suggested fix";
			fix.append(fixLabel, document.createTextNode(` · ${finding.suggested_fix}`));
			card.append(fix);
		}
		target.append(card);
	});
}
async function openArtifact(artifactId) {
	if (!artifactId) return;
	const generation = state.generation;
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	});
	try {
		const artifact = await api(`/v1/artifacts/${encodeURIComponent(artifactId)}?${query}`);
		if (!isCurrent(generation)) return;
		openDrawer("inspector");
		const target = $("tool-result");
		target.className = "tool-result artifact-detail";
		target.replaceChildren();
		const heading = document.createElement("div");
		heading.className = "artifact-detail-heading";
		const title = document.createElement("strong");
		title.textContent = artifact.metadata.title;
		const type = document.createElement("span");
		type.textContent = artifact.metadata.media_type;
		heading.append(title, type);
		target.append(heading);
		if (artifact.metadata.media_type === "application/vnd.opencoding.review+json") renderReviewReport(target, artifact.content);
		else if (artifact.metadata.media_type === "text/markdown" && typeof artifact.content === "string") {
			const body = document.createElement("div");
			body.className = "message-body";
			appendMarkdownBlocks(body, artifact.content);
			target.append(body);
		} else {
			const content = typeof artifact.content === "string" ? artifact.content : JSON.stringify(artifact.content, null, 2);
			target.append(createCodeBlock(content, artifact.metadata.media_type === "application/json" ? "json" : "text"));
		}
		announce(`Opened artifact ${artifact.metadata.title}`);
	} catch (error) {
		if (isCurrent(generation)) setToolMessage(error.message, true);
	}
}
async function loadMessages() {
	if (!state.session) return;
	const generation = state.generation;
	const sessionId = state.session.id;
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	});
	if (state.capabilities.has("transcript.snapshot")) {
		const snapshot = parseTranscriptSnapshot(await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/snapshot?${query}`));
		if (!isCurrent(generation) || state.session?.id !== sessionId) return;
		renderTranscriptSnapshot(snapshot);
		return;
	}
	const messages = await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/messages?${query}`);
	if (!isCurrent(generation) || state.session?.id !== sessionId) return;
	$("messages").replaceChildren();
	state.itemsById.clear();
	messages.forEach((message) => renderMessage(message.role, message.content, {
		itemId: message.id,
		turnId: message.turn_id
	}));
	updateConversationState(messages.length > 0);
}
function renderTranscriptSnapshot(snapshot, mergeOlder = false, preserveWindow = false) {
	if (state.session && snapshot.session.id !== state.session.id) throw new Error("transcript snapshot does not match the selected session");
	if (!mergeOlder && isTranscriptSnapshotStale(transcriptProjection, snapshot)) return;
	const preserveNewerLiveUsage = mergeOlder && snapshot.snapshot_revision < transcriptProjection.snapshotRevision;
	if (mergeOlder && loadedTranscriptSnapshot && loadedTranscriptSnapshot.session.id === snapshot.session.id) {
		const items = new Map([...snapshot.items, ...loadedTranscriptSnapshot.items].map((item) => [item.id, item]));
		snapshot = {
			...loadedTranscriptSnapshot,
			items: [...items.values()].sort((left, right) => left.created_at.localeCompare(right.created_at) || left.id.localeCompare(right.id)),
			item_count: Math.max(snapshot.item_count, loadedTranscriptSnapshot.item_count),
			next_cursor: snapshot.next_cursor,
			cursor: Math.max(snapshot.cursor, loadedTranscriptSnapshot.cursor),
			snapshot_revision: Math.max(snapshot.snapshot_revision, loadedTranscriptSnapshot.snapshot_revision)
		};
	}
	if (!preserveWindow) transcriptWindowStart = mergeOlder ? 0 : Math.max(0, snapshot.items.length - TRANSCRIPT_WINDOW_SIZE);
	transcriptWindowStart = Math.min(transcriptWindowStart, Math.max(0, snapshot.items.length - TRANSCRIPT_WINDOW_SIZE));
	loadedTranscriptSnapshot = snapshot;
	if (!preserveNewerLiveUsage) state.usage = snapshot.usage || {
		input_tokens: 0,
		output_tokens: 0,
		total_tokens: 0,
		model_calls: 0,
		tool_calls: 0,
		turns: 0
	};
	state.usageTurns = new Set(snapshot.items.filter((item) => item.kind === "usage" && item.content.type === "usage").map((item) => item.turn_id));
	$("load-earlier").hidden = !snapshot.next_cursor;
	$("load-earlier").textContent = snapshot.next_cursor ? `Load earlier · ${snapshot.items.length}/${snapshot.item_count}` : "All history loaded";
	transcriptProjection = applyTranscriptSnapshot(transcriptProjection, snapshot);
	$("messages").replaceChildren();
	$("approvals").replaceChildren();
	state.itemsById.clear();
	state.toolSteps.clear();
	state.approvals.clear();
	state.questions.clear();
	state.after = Math.max(state.after, Number(snapshot.cursor || 0));
	const visibleItems = snapshot.items.slice(transcriptWindowStart, transcriptWindowStart + TRANSCRIPT_WINDOW_SIZE);
	const transcriptWindowControl = (label, nextStart, position) => {
		const control = document.createElement("button");
		control.type = "button";
		control.className = "transcript-window-control";
		control.dataset.windowPosition = position;
		control.textContent = label;
		control.addEventListener("click", () => {
			if (!loadedTranscriptSnapshot) return;
			transcriptWindowStart = nextStart;
			renderTranscriptSnapshot(loadedTranscriptSnapshot, false, true);
			document.querySelector(`.transcript-window-control[data-window-position="${position}"]`)?.focus();
		});
		$("messages").append(control);
	};
	if (transcriptWindowStart > 0) transcriptWindowControl(`Show previous ${Math.min(TRANSCRIPT_WINDOW_SIZE, transcriptWindowStart)} loaded items`, Math.max(0, transcriptWindowStart - TRANSCRIPT_WINDOW_SIZE), "before");
	visibleItems.forEach((item) => {
		const content = item.content;
		if (isMessageItem(item)) {
			renderMessage(item.content.role || (item.kind === "user_message" ? "user" : "assistant"), item.content.content, transcriptItemIdentity(item), item.content.attachments || []);
			return;
		}
		if (item.kind === "approval" && content.type === "approval") {
			const request = content.request;
			const requestId = request.id || item.approval_id;
			if (request.status === "pending" && requestId) renderApproval(requestId, request.summary || request.tool, item.turn_id);
			return;
		}
		if (item.kind === "plan" && content.type === "plan") {
			renderPlan(item.id, item.turn_id, content.title, content.steps, item.status);
			return;
		}
		if (item.kind === "question" && content.type === "question") {
			renderQuestion(content.request, item.id, item.turn_id);
			return;
		}
		if (item.kind === "artifact" && content.type === "artifact") {
			renderArtifact(item.id, item.turn_id, content.artifact_id, content.title, content.media_type);
			return;
		}
		if (item.kind === "reasoning_summary" && content.type === "reasoning_summary") {
			renderTranscriptNotice(item.id, item.turn_id, item.kind, "Reasoning summary", content.text);
			return;
		}
		if (item.kind === "warning" && content.type === "warning") {
			renderTranscriptNotice(item.id, item.turn_id, item.kind, item.summary || "Warning", content.message);
			return;
		}
		if (item.kind === "context_compaction" && content.type === "context_compaction") {
			const detail = [
				`${content.omitted_messages} earlier message${content.omitted_messages === 1 ? "" : "s"} summarized`,
				content.truncated_messages ? `${content.truncated_messages} long message${content.truncated_messages === 1 ? "" : "s"} shortened` : "",
				content.estimated_tokens ? `about ${content.estimated_tokens.toLocaleString()} summary tokens` : ""
			].filter(Boolean).join(" · ");
			renderTranscriptNotice(item.id, item.turn_id, item.kind, "Context optimized", detail);
			return;
		}
		if (item.kind === "model_reroute" && content.type === "model_reroute") {
			const route = content.from_model ? `${content.from_model} → ${content.to_model}` : content.to_model;
			renderTranscriptNotice(item.id, item.turn_id, item.kind, "Model switched", `${route} · ${content.reason}`);
			return;
		}
		if (item.kind === "usage" && content.type === "usage") {
			renderTranscriptNotice(item.id, item.turn_id, item.kind, "Usage", `${content.total_tokens.toLocaleString()} tokens · ${content.input_tokens.toLocaleString()} input + ${content.output_tokens.toLocaleString()} output · ${content.model_calls} model / ${content.tool_calls} tool calls · ${content.model}`);
			return;
		}
		if (item.kind === "agent_status" && content.type === "agent_status") {
			renderTranscriptNotice(item.id, item.turn_id, item.kind, "Agent", content.label);
			return;
		}
		if (item.kind === "hook" && content.type === "hook") {
			const detail = [
				`${content.event} · ${content.handler}`,
				content.input_modified ? "input modified" : "",
				content.result_summary || ""
			].filter(Boolean).join(" · ");
			renderTranscriptNotice(item.id, item.turn_id, item.kind, "Hook", detail);
			return;
		}
		if (isToolItem(item)) {
			const kind = item.status === "completed" ? "tool.completed" : item.status === "failed" ? "tool.failed" : item.status === "denied" ? "tool.denied" : item.status === "awaiting_approval" ? "approval.required" : "tool.running";
			const tool = item.content.type === "mcp_call" ? item.content.namespaced_tool : item.content.tool;
			const progress = item.content.type === "mcp_call" ? item.content.progress : null;
			renderToolStep(kind, {
				tool,
				display: item.summary,
				tool_call_id: item.content.tool_call_id || item.id,
				progress: progress?.progress,
				total: progress?.total,
				message: progress?.message
			}, {
				item_id: item.id,
				turn_id: item.turn_id
			});
		}
	});
	const remainingItems = snapshot.items.length - transcriptWindowStart - visibleItems.length;
	if (remainingItems > 0) transcriptWindowControl(`Show next ${Math.min(TRANSCRIPT_WINDOW_SIZE, remainingItems)} loaded items`, transcriptWindowStart + TRANSCRIPT_WINDOW_SIZE, "after");
	state.pendingInputs = snapshot.pending_inputs || [];
	renderPendingInputs();
	const activeTurn = snapshot.turns.findLast((turn) => ![
		"completed",
		"failed",
		"cancelled"
	].includes(turn.status));
	if (activeTurn) {
		state.turn = activeTurn.id;
		setTurnRunning(true);
		$("turn-state").textContent = activeTurn.status;
	} else {
		state.turn = snapshot.turns.at(-1)?.id || null;
		setTurnRunning(false);
	}
	updateConversationState(snapshot.items.length > 0);
}
async function loadEarlierTranscript() {
	const session = state.session;
	const cursor = loadedTranscriptSnapshot?.next_cursor;
	if (!session || !cursor) return;
	if (state.turnRunning) {
		toast("Wait for the running task to finish before loading older history.");
		return;
	}
	const query = catalogQuery();
	query.set("before", cursor);
	query.set("limit", "500");
	$("load-earlier").disabled = true;
	try {
		const snapshot = parseTranscriptSnapshot(await api(`/v1/sessions/${encodeURIComponent(session.id)}/snapshot?${query}`));
		if (state.session?.id !== session.id) return;
		renderTranscriptSnapshot(snapshot, true);
	} finally {
		$("load-earlier").disabled = false;
	}
}
async function executeContent(content, files = []) {
	const session = state.session;
	if (!session) throw new Error("No active session");
	const shellCommand = content.startsWith("!") ? content.slice(1).trim() : null;
	const generation = state.generation;
	let uploaded = [];
	if (files.length) {
		$("turn-state").textContent = "uploading";
		uploaded = await uploadDraftAttachments(session.id, files);
	}
	const optimisticMessage = renderMessage("user", content, {}, uploaded);
	$("turn-state").textContent = "starting";
	$("send-turn").disabled = true;
	try {
		if (shellCommand !== null) {
			const outcome = await api(`/v1/sessions/${encodeURIComponent(session.id)}/tools`, {
				method: "POST",
				body: JSON.stringify({
					scope: scope(),
					tool: "run_command",
					arguments: {
						program: "sh",
						args: ["-lc", shellCommand],
						timeout_seconds: 60,
						network_enabled: false,
						max_bytes: 1048576
					}
				})
			});
			if (!isCurrent(generation)) return;
			state.turn = outcome.tool_call?.request?.turn_id || null;
			setTurnRunning(outcome.outcome === "awaiting_approval");
			$("turn-state").textContent = outcome.outcome || "submitted";
			if (outcome.outcome === "completed") renderToolOutcome(outcome);
		} else {
			const turn = await api(`/v1/sessions/${encodeURIComponent(session.id)}/turns`, {
				method: "POST",
				body: JSON.stringify({
					scope: scope(),
					content,
					attachment_ids: uploaded.map((attachment) => attachment.id)
				})
			});
			if (!isCurrent(generation)) return;
			state.draftFiles = [];
			state.draftFilesByContext.set(composerDraftContext(), []);
			$("attachment-input").value = "";
			renderDraftAttachments();
			state.turn = turn.id;
			setTurnRunning(true);
			$("undo-turn").disabled = true;
			$("turn-state").textContent = turn.status;
		}
	} catch (error) {
		optimisticMessage.remove();
		updateConversationState();
		await Promise.allSettled(uploaded.map((attachment) => deleteDraftAttachment(attachment.id)));
		if (isCurrent(generation)) {
			if (shellCommand === null && files.length === 0 && state.capabilities.has("turn.input_queue.v1") && error instanceof Error && error.message === "session already has active or queued input") try {
				await loadMessages();
				if (isCurrent(generation) && state.session?.id === session.id && state.turnRunning && state.turn) {
					await submitTurnInput(content, "queue");
					return;
				}
			} catch (_) {}
			if (!$("prompt").value) {
				$("prompt").value = content;
				sessionStorage.setItem(composerTextDraftKey(), content);
				resizePrompt();
			}
			addActivity("turn.error", { error: error.message });
		}
	} finally {
		if (isCurrent(generation)) updateSendAction();
	}
}
async function runTurn(event) {
	event.preventDefault();
	if (composerSubmissionPending) return;
	closeMentionMenu();
	const text = $("prompt").value.trim();
	const hasAttachments = state.draftFiles.length > 0;
	const content = text || (hasAttachments ? "Please inspect the attached files." : "");
	if (state.turnRunning) {
		if (hasAttachments) {
			toast("Attachments cannot be added to a running Turn yet. Remove them or wait for completion.");
			return;
		}
		if (content) await withComposerSubmission(async () => {
			try {
				await submitTurnInput(content, "queue");
			} catch (error) {
				addActivity("turn.input.queue.error", { error: error.message });
			}
		});
		else await withComposerSubmission(cancelActiveTurn);
		return;
	}
	if (!content) return;
	const shellCommand = content.startsWith("!") ? content.slice(1).trim() : null;
	if (shellCommand === "") {
		toast("Enter a command after !");
		return;
	}
	if (shellCommand !== null && hasAttachments) {
		toast("Attachments are available to agent messages, not shell commands.");
		return;
	}
	if (shellCommand === null && !state.capabilities.has("agent.tool_loop")) {
		addActivity("agent.unavailable", { reason: "Connect the daemon before starting a task" });
		openDrawer("settings-drawer");
		return;
	}
	if (!state.session && !await createSession()) return;
	$("prompt").value = "";
	sessionStorage.removeItem(composerTextDraftKey());
	resizePrompt();
	updateSendAction();
	await withComposerSubmission(() => executeContent(content, [...state.draftFiles]));
}
function activityLabel(kind) {
	return {
		"approval.required": "Decision needed",
		"session.updated": "Session updated",
		"session.cancelled": "Session cancelled",
		"session.deleted": "Session deleted",
		"turn.completed": "Task completed",
		"turn.failed": "Task failed",
		"turn.cancelled": "Task stopped",
		"turn.status": "Task progress",
		"tool.completed": "Tool completed",
		"tool.failed": "Tool failed",
		"tool.denied": "Tool denied"
	}[kind] || String(kind).split(".").map((part) => part.charAt(0).toUpperCase() + part.slice(1)).join(" · ");
}
function activityDetail(payload) {
	if (!payload || typeof payload !== "object") return "Update received";
	for (const key of [
		"error",
		"error_code",
		"reason",
		"status",
		"tool",
		"title",
		"message"
	]) if (typeof payload[key] === "string" && payload[key].trim()) return key === "tool" ? friendlyTool(payload[key]) : payload[key];
	const identifier = payload.session_id || payload.turn_id || payload.task_id || payload.approval_id;
	return identifier ? `Reference ${String(identifier).slice(0, 12)}` : "Update received";
}
function friendlyTool(tool) {
	return {
		read_file: "Read file",
		search_text: "Search code",
		apply_patch: "Edit files",
		git_diff: "Review changes",
		shell: "Run shell"
	}[tool] || String(tool).replaceAll("_", " ");
}
function shouldRenderToolStep(kind) {
	return kind === "approval.required" || kind.startsWith("tool.") || kind === "mcp.progress" || [
		"turn.created",
		"turn.status",
		"turn.awaiting_input",
		"turn.awaiting_approval",
		"turn.completed",
		"turn.failed",
		"turn.cancelled"
	].includes(kind) || kind.endsWith(".error");
}
function renderToolStep(kind, payload, envelope = {}) {
	if (!shouldRenderToolStep(kind)) return;
	if (payload?.tool === "update_plan" || payload?.tool === "request_user_input" || payload?.tool === "publish_artifact") return;
	const itemId = payload?.tool_call_id || payload?.model_call_id || envelope.item_id || null;
	const turnId = envelope.turn_id || null;
	const toolEvent = kind.startsWith("tool.") || kind === "approval.required" || kind === "mcp.progress";
	const family = toolEvent ? `tool:${itemId || `${turnId || "unknown"}:${payload?.tool || "tool"}`}` : kind.startsWith("turn.") ? `turn:${turnId || "unknown"}` : `${kind}:${itemId || turnId || "unknown"}`;
	let item = state.toolSteps.get(family);
	if (!item && toolEvent) {
		const pending = [...state.toolSteps.entries()].find(([, candidate]) => candidate.dataset.turnId === (turnId || "") && candidate.dataset.tool === (payload?.tool || "tool") && !candidate.classList.contains("complete") && !candidate.classList.contains("error"));
		if (pending) {
			const [previousFamily, candidate] = pending;
			state.toolSteps.delete(previousFamily);
			state.toolSteps.set(family, candidate);
			item = candidate;
		}
	}
	if (!item?.isConnected) {
		item = document.createElement("div");
		item.className = "tool-step";
		if (itemId) item.dataset.itemId = itemId;
		if (turnId) item.dataset.turnId = turnId;
		item.dataset.tool = payload?.tool || "tool";
		item.setAttribute("role", "status");
		const marker = document.createElement("span");
		marker.className = "tool-step-marker";
		marker.setAttribute("aria-hidden", "true");
		const copy = document.createElement("span");
		copy.className = "tool-step-copy";
		const title = document.createElement("strong");
		const detail = document.createElement("span");
		copy.append(title, detail);
		item.append(marker, copy);
		const streamingAnswer = turnId ? [...$("messages").querySelectorAll(".message.assistant.streaming")].find((candidate) => candidate.dataset.turnId === turnId) : null;
		if (toolEvent && streamingAnswer) $("messages").insertBefore(item, streamingAnswer);
		else $("messages").append(item);
		state.toolSteps.set(family, item);
		if (itemId) state.itemsById.set(itemId, item);
	}
	if (itemId) {
		item.dataset.itemId = itemId;
		state.itemsById.set(itemId, item);
	}
	if (typeof payload?.display === "string" && payload.display.trim()) item.dataset.display = payload.display;
	item.classList.toggle("error", /(error|failed)/.test(kind));
	item.classList.toggle("decision", /(approval|denied|policy)/.test(kind));
	item.classList.toggle("complete", /(completed|cancelled)/.test(kind));
	const title = item.querySelector("strong");
	const detail = item.querySelector(".tool-step-copy > span");
	if (title) title.textContent = activityLabel(kind);
	if (detail) detail.textContent = toolEvent ? toolStepDetail(payload, item.dataset.display) : activityDetail(payload);
	updateConversationState(true);
}
function toolStepDetail(payload, preservedDisplay) {
	const base = typeof payload?.display === "string" && payload.display.trim() ? payload.display : preservedDisplay || friendlyTool(payload?.tool || "tool");
	if (typeof payload?.progress !== "number") return base;
	return [
		base,
		typeof payload.total === "number" && payload.total > 0 ? `${Math.max(0, Math.min(100, Math.round(payload.progress / payload.total * 100)))}%` : String(payload.progress),
		typeof payload.message === "string" ? payload.message : ""
	].filter(Boolean).join(" · ");
}
function addActivity(kind, payload, envelope = {}) {
	if (!state.connected || kind === "model.delta") return;
	renderToolStep(kind, payload, envelope);
	$("activity").classList.remove("empty");
	if ($("activity").textContent === "No activity yet") $("activity").replaceChildren();
	const item = document.createElement("div");
	item.className = `event${/(error|failed)/.test(kind) ? " error" : /(approval|denied|policy)/.test(kind) ? " decision" : ""}`;
	const title = document.createElement("strong");
	title.textContent = activityLabel(kind);
	const detail = document.createElement("span");
	detail.textContent = activityDetail(payload);
	const time = document.createElement("time");
	time.dateTime = (/* @__PURE__ */ new Date()).toISOString();
	time.textContent = "just now";
	item.append(title, detail, time);
	$("activity").prepend(item);
	while ($("activity").children.length > 100) $("activity").lastChild?.remove();
}
function renderApproval(id, tool, turnId = null) {
	if (!id || state.approvals.has(id)) return;
	state.approvals.add(id);
	const row = document.createElement("div");
	row.className = "approval";
	row.dataset.id = id;
	row.dataset.itemId = id;
	if (turnId) row.dataset.turnId = turnId;
	const label = document.createElement("span");
	label.textContent = `${tool || "Tool"} needs permission to continue`;
	const actions = document.createElement("div");
	[
		{
			label: "Allow once",
			approved: true,
			scope: "once"
		},
		{
			label: "Allow this session",
			approved: true,
			scope: "session"
		},
		{
			label: "Reject",
			approved: false,
			scope: "once"
		}
	].forEach((choice) => {
		const button = document.createElement("button");
		button.textContent = choice.label;
		button.classList.toggle("primary", choice.approved && choice.scope === "once");
		button.addEventListener("click", () => resolveApproval(id, choice.approved, row, choice.scope));
		actions.append(button);
	});
	row.append(label, actions);
	$("approvals").append(row);
	state.itemsById.set(id, row);
	announce(`Approval required for ${tool || "tool"}`);
}
async function resolveApproval(id, approved, row, approvalScope = "once") {
	const generation = state.generation;
	try {
		await api(`/v1/approvals/${encodeURIComponent(id)}`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				approved,
				approval_scope: approvalScope
			})
		});
		if (!isCurrent(generation)) return;
		row.remove();
		state.approvals.delete(id);
	} catch (error) {
		if (isCurrent(generation)) addActivity("approval.error", { error: error.message });
	}
}
function renderQuestion(request, itemId = null, turnId = null) {
	if (!request?.id || !Array.isArray(request.questions)) return null;
	const stableItemId = itemId || request.item_id || `question_${request.id}`;
	const existing = state.itemsById.get(stableItemId);
	const previousTimer = existing?._countdownTimer;
	if (typeof previousTimer === "number") window.clearInterval(previousTimer);
	const card = document.createElement("article");
	card.className = "question-card";
	card.dataset.itemId = stableItemId;
	card.dataset.requestId = request.id;
	if (turnId || request.turn_id) card.dataset.turnId = turnId || request.turn_id;
	const heading = document.createElement("div");
	heading.className = "question-heading";
	const title = document.createElement("strong");
	title.textContent = request.status === "answered" ? "Answered" : "Your input is needed";
	const count = document.createElement("span");
	count.textContent = `${request.questions.length} question${request.questions.length === 1 ? "" : "s"}`;
	heading.append(title, count);
	if (request.status !== "answered" && request.expires_at) {
		const deadline = Date.parse(request.expires_at);
		if (Number.isFinite(deadline)) {
			const countdown = document.createElement("span");
			countdown.className = "question-countdown";
			const updateCountdown = () => {
				const seconds = Math.max(0, Math.ceil((deadline - Date.now()) / 1e3));
				countdown.textContent = seconds > 0 ? `Recommended defaults in ${seconds}s` : "Applying recommended defaults…";
				if (seconds <= 0 && typeof card._countdownTimer === "number") {
					window.clearInterval(card._countdownTimer);
					delete card._countdownTimer;
				}
			};
			updateCountdown();
			card._countdownTimer = window.setInterval(updateCountdown, 1e3);
			heading.append(countdown);
		}
	}
	card.append(heading);
	if (request.status === "answered") request.questions.forEach((prompt) => {
		const row = document.createElement("div");
		row.className = "question-resolved";
		const label = document.createElement("strong");
		label.textContent = prompt.header;
		const answer = request.answers?.find((value) => value.question_id === prompt.id);
		const value = document.createElement("span");
		value.textContent = answer?.answer || "Answered in another client";
		row.append(label, value);
		card.append(row);
	});
	else {
		const form = document.createElement("form");
		form.className = "question-form";
		request.questions.forEach((prompt, questionIndex) => {
			const fieldset = document.createElement("fieldset");
			fieldset.dataset.questionId = prompt.id;
			const legend = document.createElement("legend");
			const eyebrow = document.createElement("span");
			eyebrow.textContent = prompt.header;
			const promptText = document.createElement("strong");
			promptText.textContent = prompt.question;
			legend.append(eyebrow, promptText);
			fieldset.append(legend);
			(prompt.options || []).forEach((option, optionIndex) => {
				const label = document.createElement("label");
				label.className = "question-option";
				const radio = document.createElement("input");
				radio.type = "radio";
				radio.name = `question-${request.id}-${questionIndex}`;
				radio.value = option.label;
				if (optionIndex === 0) radio.dataset.first = "true";
				const copy = document.createElement("span");
				const name = document.createElement("strong");
				name.textContent = option.label;
				const description = document.createElement("small");
				description.textContent = option.description;
				copy.append(name, description);
				label.append(radio, copy);
				fieldset.append(label);
			});
			if (request.allow_other !== false) {
				const other = document.createElement("input");
				other.className = "question-other";
				other.type = "text";
				other.maxLength = 500;
				other.placeholder = "Or type another answer";
				other.setAttribute("aria-label", `${prompt.header} other answer`);
				fieldset.append(other);
			}
			form.append(fieldset);
		});
		const error = document.createElement("p");
		error.className = "question-error";
		error.setAttribute("role", "alert");
		const submit = document.createElement("button");
		submit.type = "submit";
		submit.className = "primary";
		submit.textContent = "Submit answer";
		form.append(error, submit);
		form.addEventListener("submit", async (event) => {
			event.preventDefault();
			const answers = [];
			for (const fieldset of form.querySelectorAll("fieldset")) {
				const selected = fieldset.querySelector("input[type=\"radio\"]:checked");
				const answer = fieldset.querySelector(".question-other")?.value.trim() || selected?.value || "";
				if (!answer) {
					error.textContent = "Answer each question before continuing.";
					return;
				}
				const questionId = fieldset.dataset.questionId;
				if (!questionId) {
					error.textContent = "This question is missing its identifier.";
					return;
				}
				answers.push({
					question_id: questionId,
					answer
				});
			}
			error.textContent = "";
			submit.disabled = true;
			const generation = state.generation;
			try {
				const answered = await api(`/v1/questions/${encodeURIComponent(request.id)}`, {
					method: "POST",
					body: JSON.stringify({
						scope: scope(),
						answers
					})
				});
				if (!isCurrent(generation)) return;
				renderQuestion(answered, stableItemId, turnId || request.turn_id);
				$("turn-state").textContent = "continuing";
			} catch (submitError) {
				if (isCurrent(generation)) {
					error.textContent = submitError.message;
					submit.disabled = false;
				}
			}
		});
		card.append(form);
	}
	if (existing) existing.replaceWith(card);
	else $("messages").append(card);
	state.itemsById.set(stableItemId, card);
	state.questions.add(request.id);
	updateConversationState(true);
	if (request.status !== "answered") announce("Your input is needed");
	return card;
}
function markQuestionAnswered(requestId, itemId = null) {
	const card = itemId ? state.itemsById.get(itemId) : $("messages").querySelector(`[data-request-id="${CSS.escape(requestId || "")}"]`);
	if (!card) return;
	if (typeof card._countdownTimer === "number") {
		window.clearInterval(card._countdownTimer);
		delete card._countdownTimer;
	}
	card.classList.add("resolved");
	const heading = card.querySelector(".question-heading strong");
	if (heading) heading.textContent = "Answered in another client";
	card.querySelectorAll("input, button").forEach((control) => {
		control.disabled = true;
	});
}
async function showDiff() {
	if (!state.session) {
		toast("Start or select a task to view changes");
		return;
	}
	const generation = state.generation;
	setToolMessage("Loading changes…");
	try {
		const outcome = await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/tools`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				tool: "git_diff",
				arguments: {
					paths: [],
					max_bytes: 524288
				}
			})
		});
		if (isCurrent(generation)) {
			renderToolOutcome(outcome);
			announce("Changes loaded");
		}
	} catch (error) {
		if (isCurrent(generation)) setToolMessage(error.message, true);
	}
}
function setToolMessage(message, error = false) {
	const target = $("tool-result");
	target.className = `tool-result ${error ? "error" : "empty"}`;
	target.replaceChildren(document.createTextNode(message));
}
function toolResult(outcome) {
	return outcome?.tool_call?.result ?? outcome?.result ?? outcome;
}
function renderDiffSnapshot(snapshot) {
	const target = $("tool-result");
	const diff = String(snapshot.unified_diff || "");
	if (!diff) {
		setToolMessage("Working tree is clean — no changes to review.");
		return;
	}
	const lines = diff.split("\n");
	const additions = lines.filter((line) => line.startsWith("+") && !line.startsWith("+++")).length;
	const deletions = lines.filter((line) => line.startsWith("-") && !line.startsWith("---")).length;
	const files = lines.filter((line) => line.startsWith("diff --git ")).length;
	target.className = "tool-result diff-result";
	target.replaceChildren();
	const summary = document.createElement("div");
	summary.className = "diff-summary";
	const identity = document.createElement("div");
	const title = document.createElement("strong");
	title.textContent = `${files || 1} changed ${files === 1 ? "file" : "files"}`;
	const hash = document.createElement("small");
	hash.textContent = `${snapshot.truncated ? "Partial diff" : "Complete diff"} · ${String(snapshot.sha256 || "no hash").slice(0, 12)}`;
	identity.append(title, hash);
	const added = document.createElement("span");
	added.className = "diff-stat add";
	added.textContent = `+${additions}`;
	const removed = document.createElement("span");
	removed.className = "diff-stat delete";
	removed.textContent = `−${deletions}`;
	const copy = document.createElement("button");
	copy.type = "button";
	copy.className = "code-copy";
	copy.textContent = "Copy";
	copy.setAttribute("aria-label", "Copy unified diff");
	copy.addEventListener("click", () => copyText(diff, "Diff copied"));
	summary.append(identity, added, removed, copy);
	const pre = document.createElement("pre");
	pre.className = "diff-code";
	pre.setAttribute("aria-label", "Unified diff");
	lines.forEach((line) => {
		const row = document.createElement("span");
		row.className = `diff-line${line.startsWith("diff --git") || line.startsWith("---") || line.startsWith("+++") ? " header" : line.startsWith("@@") ? " hunk" : line.startsWith("+") ? " add" : line.startsWith("-") ? " delete" : ""}`;
		row.textContent = line || " ";
		pre.append(row);
	});
	target.append(summary, pre);
}
function renderToolOutcome(outcome) {
	const result = toolResult(outcome);
	if (isJsonObject(result) && typeof result.unified_diff === "string") {
		renderDiffSnapshot(result);
		return;
	}
	const target = $("tool-result");
	target.className = "tool-result structured";
	target.replaceChildren();
	const pre = document.createElement("pre");
	pre.className = "structured-result";
	pre.textContent = typeof result === "string" ? result : JSON.stringify(result, null, 2);
	target.append(pre);
}
async function undoTurn(turnId = state.turn) {
	if (!turnId || !state.capabilities.has("turn.undo")) return;
	const generation = state.generation;
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	});
	let impact;
	try {
		impact = await api(`/v1/turns/${encodeURIComponent(turnId)}/undo-impact?${query}`);
	} catch (error) {
		if (isCurrent(generation)) setToolMessage(`Unable to preview rollback impact: ${error.message}`, true);
		return;
	}
	const details = impact.paths.length ? impact.paths.map((path) => `${path.action === "delete" ? "Delete created file" : "Restore previous content"}: ${path.path}`) : ["No file changes were recorded for this turn"];
	if (impact.conversation_messages) details.push(`Remove ${impact.conversation_messages} message(s) from future model context`);
	if (impact.plan_items) details.push(`Rewind ${impact.plan_items} plan item(s)`);
	if (impact.queued_inputs) details.push(`Cancel ${impact.queued_inputs} queued follow-up input(s)`);
	if (impact.session_goal_changes) details.push("Restore the Session Goal state before this turn");
	if (!await requestAction({
		eyebrow: "Safe rollback",
		title: "Undo this turn?",
		description: "Conversation-time state is rewound together. Audit, usage and external side effects remain recorded.",
		details,
		confirm: "Undo turn",
		danger: true
	})) return;
	try {
		const result = await api(`/v1/turns/${encodeURIComponent(turnId)}/undo`, {
			method: "POST",
			body: JSON.stringify({ scope: scope() })
		});
		if (isCurrent(generation)) {
			renderToolOutcome(result);
			await Promise.all([loadMessages(), loadSessionGoal()]);
			if (turnId === state.turn) $("undo-turn").disabled = true;
			toast("Turn and conversation state restored");
		}
	} catch (error) {
		if (isCurrent(generation)) setToolMessage(error.message, true);
	}
}
async function showCheckpoints() {
	if (!state.session) return;
	const generation = state.generation;
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	});
	try {
		const turns = await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/turns?${query}`);
		if (!isCurrent(generation)) return;
		const target = $("tool-result");
		target.replaceChildren();
		target.className = `tool-result checkpoint-list${turns.length ? "" : " empty"}`;
		if (!turns.length) {
			target.textContent = "No checkpoints in this session.";
			return;
		}
		[...turns].reverse().forEach((turn, index) => {
			const row = document.createElement("article");
			row.className = "checkpoint-row";
			const detail = document.createElement("div");
			const title = document.createElement("strong");
			title.textContent = index === 0 ? "Latest turn" : `Checkpoint ${turns.length - index}`;
			const meta = document.createElement("small");
			meta.textContent = `${turn.status} · ${turn.id}`;
			detail.append(title, meta);
			const undo = document.createElement("button");
			undo.type = "button";
			undo.textContent = "Undo";
			undo.disabled = !state.capabilities.has("turn.undo") || turn.status !== "completed";
			undo.addEventListener("click", () => undoTurn(turn.id));
			row.append(detail, undo);
			target.append(row);
		});
	} catch (error) {
		if (isCurrent(generation)) setToolMessage(error.message, true);
	}
}
async function showBranches() {
	if (!state.session) return;
	const selectedId = state.session.id;
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	});
	try {
		const tree = await api(`/v1/sessions/${encodeURIComponent(selectedId)}/branches?${query}`);
		if (state.session?.id !== selectedId) return;
		const target = $("tool-result");
		target.replaceChildren();
		target.className = "tool-result branch-list";
		const byId = new Map(tree.nodes.map((node) => [node.session.id, node]));
		const depth = (node, seen = /* @__PURE__ */ new Set()) => {
			if (!node.parent_session_id || seen.has(node.session.id)) return 0;
			seen.add(node.session.id);
			const parent = byId.get(node.parent_session_id);
			return parent ? 1 + depth(parent, seen) : 0;
		};
		tree.nodes.forEach((node) => {
			const row = document.createElement("article");
			row.className = "branch-row";
			row.style.setProperty("--branch-indent", `${Math.min(depth(node), 6) * 18}px`);
			const marker = document.createElement("span");
			marker.className = "branch-marker";
			marker.setAttribute("aria-hidden", "true");
			const detail = document.createElement("div");
			const title = document.createElement("strong");
			title.textContent = node.session.title;
			const meta = document.createElement("small");
			const current = node.session.id === selectedId ? "Current · " : "";
			const source = node.source_turn_id ? ` · after ${node.source_turn_id}` : "";
			meta.textContent = `${current}${node.session.status} · ${node.session.model}${source}`;
			detail.append(title, meta);
			const open = document.createElement("button");
			open.type = "button";
			open.textContent = node.session.id === selectedId ? "Open" : "Switch";
			open.disabled = node.session.id === selectedId;
			open.addEventListener("click", () => selectSession(node.session));
			row.append(marker, detail, open);
			target.append(row);
		});
		if (!tree.nodes.length) {
			target.classList.add("empty");
			target.textContent = "No branches are available.";
		}
	} catch (error) {
		toast(error.message);
	}
}
async function exportSession() {
	if (!state.session) return;
	const sessionId = state.session.id;
	const values = await requestAction({
		eyebrow: "Session",
		title: "Export transcript",
		description: "Download a point-in-time copy of the visible transcript. Attachment contents are not embedded.",
		confirm: "Download",
		fields: [{
			name: "format",
			label: "Format",
			options: [["markdown", "Markdown"], ["json", "JSON snapshot"]]
		}]
	});
	if (!values || state.session?.id !== sessionId) return;
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id,
		format: values.format || "markdown"
	});
	try {
		const exported = await api(`/v1/sessions/${encodeURIComponent(sessionId)}/export?${query}`);
		if (state.session?.id !== sessionId) return;
		const blob = new Blob([exported.content], { type: `${exported.media_type};charset=utf-8` });
		const href = URL.createObjectURL(blob);
		const link = document.createElement("a");
		link.href = href;
		link.download = exported.file_name;
		document.body.append(link);
		link.click();
		link.remove();
		window.setTimeout(() => URL.revokeObjectURL(href), 0);
		toast(`Transcript exported · ${exported.sha256.slice(0, 12)}`);
	} catch (error) {
		toast(error.message);
	}
}
async function createMemory() {
	if (!state.session) return;
	const values = await requestAction({
		eyebrow: "Context memory",
		title: "Remember this context",
		description: "Memory is encrypted at rest, requires a citation, and rejects likely secrets.",
		confirm: "Save memory",
		fields: [
			{
				name: "memoryScope",
				label: "Scope",
				options: [
					["project", "This project"],
					["user", "My sessions"],
					["team", "Team"]
				]
			},
			{
				name: "citation",
				label: "Citation / source",
				placeholder: "Architecture decision ADR-012",
				required: true,
				maxlength: 500
			},
			{
				name: "content",
				label: "What should Opencoding remember?",
				multiline: true,
				required: true,
				maxlength: 16e3
			},
			{
				name: "expiry",
				label: "Expires",
				options: [
					["30", "In 30 days"],
					["7", "In 7 days"],
					["90", "In 90 days"],
					["never", "Never"]
				]
			}
		]
	});
	if (!values) return;
	const expiresAt = values.expiry === "never" ? null : new Date(Date.now() + Number(values.expiry) * 864e5).toISOString();
	try {
		await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/memories`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				memory_scope: values.memoryScope,
				citation: values.citation,
				content: values.content,
				expires_at: expiresAt
			})
		});
		toast("Memory saved with its citation");
		await showContext();
	} catch (error) {
		toast(error.message);
	}
}
async function removeMemory(memory) {
	if (!state.session) return;
	if (!await requestAction({
		eyebrow: "Context memory",
		title: "Remove this memory?",
		description: memory.citation,
		details: [`Scope: ${memory.memory_scope}`, `Source: ${memory.source_uri}`],
		confirm: "Remove memory",
		danger: true
	})) return;
	try {
		await api(`/v1/memories/${encodeURIComponent(memory.id)}?${catalogQuery()}`, { method: "DELETE" });
		toast("Memory removed");
		await showContext();
	} catch (error) {
		toast(error.message);
	}
}
async function compactContext() {
	if (!state.session) return;
	const values = await requestAction({
		eyebrow: "Context",
		title: "Compact earlier conversation?",
		description: "Recent messages stay verbatim. Earlier messages become a bounded summary; the full transcript remains available.",
		confirm: "Compact context",
		fields: [{
			name: "focus",
			label: "Preserve this focus (optional, kept for 24 hours)",
			multiline: true,
			maxlength: 500,
			placeholder: "Keep test evidence and final architecture decisions."
		}]
	});
	if (!values) return;
	try {
		const result = await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/compact`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				focus: values.focus || null
			})
		});
		toast(`Context compacted · ${result.before_tokens.toLocaleString()} → ${result.after_tokens.toLocaleString()} tokens`);
		await showContext();
	} catch (error) {
		toast(error.message);
	}
}
async function showContext() {
	if (!state.session || !state.capabilities.has("context.explain")) return;
	const generation = state.generation;
	const s = scope();
	const query = new URLSearchParams({
		organization_id: s.organization_id,
		team_id: s.team_id,
		actor_id: s.actor_id
	});
	try {
		const [summary, memories] = await Promise.all([api(`/v1/sessions/${encodeURIComponent(state.session.id)}/context?${query}`), api(`/v1/sessions/${encodeURIComponent(state.session.id)}/memories?${query}`)]);
		if (!isCurrent(generation)) return;
		const target = $("tool-result");
		target.replaceChildren();
		target.className = "tool-result context-summary";
		const heading = document.createElement("div");
		heading.className = "context-total";
		const title = document.createElement("strong");
		title.textContent = `${summary.total_estimated_tokens.toLocaleString()} estimated tokens`;
		const meta = document.createElement("small");
		meta.textContent = `${summary.conversation_tokens.toLocaleString()} conversation · ${summary.item_tokens.toLocaleString()} sources · ${summary.reserved_output_tokens.toLocaleString()} reserved output`;
		const usage = document.createElement("small");
		usage.textContent = `Session used ${state.usage.total_tokens.toLocaleString()} tokens · ${state.usage.input_tokens.toLocaleString()} input + ${state.usage.output_tokens.toLocaleString()} output · ${state.usage.model_calls} model / ${state.usage.tool_calls} tool calls`;
		heading.append(title, meta, usage);
		target.append(heading);
		const actions = document.createElement("div");
		actions.className = "context-actions";
		const compact = document.createElement("button");
		compact.type = "button";
		compact.textContent = "Compact";
		compact.addEventListener("click", compactContext);
		const remember = document.createElement("button");
		remember.type = "button";
		remember.textContent = "Add memory";
		remember.addEventListener("click", createMemory);
		actions.append(compact, remember);
		target.append(actions);
		if (!summary.items.length) {
			const empty = document.createElement("p");
			empty.textContent = "No project, Team Knowledge, or editor context is loaded.";
			target.append(empty);
		}
		summary.items.forEach((item) => {
			const row = document.createElement("article");
			row.className = "context-row";
			const identity = document.createElement("div");
			const source = document.createElement("strong");
			source.textContent = item.source_uri;
			const detail = document.createElement("small");
			detail.textContent = `${item.kind.replaceAll("_", " ")} · ${item.estimated_tokens.toLocaleString()} tokens · ${item.trust_level}${item.pinned ? " · pinned" : ""}`;
			identity.append(source, detail);
			row.append(identity);
			target.append(row);
		});
		const memoryHeading = document.createElement("div");
		memoryHeading.className = "context-section-heading";
		memoryHeading.textContent = `Memory · ${memories.length}`;
		target.append(memoryHeading);
		if (!memories.length) {
			const empty = document.createElement("p");
			empty.className = "context-empty";
			empty.textContent = "No active User, Project, or Team memory.";
			target.append(empty);
		}
		memories.forEach((memory) => {
			const row = document.createElement("article");
			row.className = "context-row memory-row";
			const identity = document.createElement("div");
			const citation = document.createElement("strong");
			citation.textContent = memory.citation;
			const detail = document.createElement("small");
			detail.textContent = `${memory.memory_scope} · ${memory.expires_at ? `expires ${new Date(memory.expires_at).toLocaleDateString()}` : "no expiry"} · ${memory.source_uri}`;
			identity.append(citation, detail);
			const remove = document.createElement("button");
			remove.type = "button";
			remove.textContent = "Remove";
			remove.setAttribute("aria-label", `Remove memory ${memory.citation}`);
			remove.addEventListener("click", () => removeMemory(memory));
			row.append(identity, remove);
			target.append(row);
		});
	} catch (error) {
		if (isCurrent(generation)) setToolMessage(error.message, true);
	}
}
async function startReview() {
	if (!state.session || !state.capabilities.has("review.read_only")) return;
	if (state.turnRunning) {
		toast("Finish or stop the current turn before starting a review");
		return;
	}
	const generation = state.generation;
	renderMessage("user", "Review uncommitted changes");
	$("turn-state").textContent = "starting review";
	try {
		const turn = await api(`/v1/sessions/${encodeURIComponent(state.session.id)}/reviews`, {
			method: "POST",
			body: JSON.stringify({
				scope: scope(),
				target: "uncommitted",
				instructions: null
			})
		});
		if (!isCurrent(generation)) return;
		state.turn = turn.id;
		setTurnRunning(true);
		$("undo-turn").disabled = true;
		$("turn-state").textContent = turn.status;
		toast("Read-only review started");
	} catch (error) {
		if (isCurrent(generation)) addActivity("review.error", { error: error.message });
	}
}
async function renderServerStartedInput(inputId, itemId, turnId, sessionId) {
	if (!inputId || !itemId || !turnId || !sessionId || state.session?.id !== sessionId) return;
	try {
		const s = scope();
		const query = new URLSearchParams({
			organization_id: s.organization_id,
			team_id: s.team_id,
			actor_id: s.actor_id
		});
		const input = await api(`/v1/turn-inputs/${encodeURIComponent(inputId)}?${query}`);
		if (state.session?.id !== sessionId || state.itemsById.has(itemId)) return;
		renderMessage("user", input.content, {
			itemId,
			turnId
		});
	} catch (error) {
		addActivity("turn.input.load.error", { error: error.message });
	}
}
function handleEvent(kind, payload, envelope = {}) {
	if (kind === "mcp.progress") renderToolStep(kind, payload, envelope);
	else if (!["turn.usage", "reasoning.summary.delta"].includes(kind)) addActivity(kind, payload, envelope);
	if (kind === "session.updated" && envelope.session_id) {
		const session = state.session;
		if (session && session.id === envelope.session_id) {
			if (payload.title) {
				session.title = payload.title;
				$("session-title").textContent = payload.title;
			}
			if (payload.status) {
				session.status = payload.status;
				$("session-meta").textContent = `${session.status} · ${session.model} · ${session.workspace_uri}`;
				$("cancel-session").textContent = session.status === "archived" ? "Restore" : "Archive";
				$("cancel-session").classList.toggle("danger", session.status !== "archived");
			}
		}
		refreshSessions().catch((error) => addActivity("session.refresh.error", { error: error.message }));
	}
	if (kind === "session.cancelled" || kind === "session.deleted") {
		if (state.session && envelope.session_id === state.session.id) if (kind === "session.deleted") clearSessionSelection();
		else {
			state.session.status = "archived";
			$("session-meta").textContent = `archived · ${state.session.model} · ${state.session.workspace_uri}`;
			$("cancel-session").disabled = false;
			$("cancel-session").textContent = "Restore";
			$("cancel-session").classList.remove("danger");
		}
		refreshSessions().catch((error) => addActivity("session.refresh.error", { error: error.message }));
	}
	const notificationMessage = {
		"approval.required": "A task needs an approval decision.",
		"question.required": "A task is waiting for your answer.",
		"turn.completed": "A task completed.",
		"turn.failed": "A task failed. Open Opencoding for details.",
		"terminal.completed": "A background terminal finished. Its output Artifact is ready."
	}[kind];
	if (notificationMessage) notifyUser(`opencoding:${envelope.session_id || "team"}:${envelope.turn_id || "none"}:${kind}`, notificationMessage);
	if (kind === "terminal.started" || kind === "terminal.completed") refreshTeam().catch((error) => addActivity("terminal.refresh.error", { error: error.message }));
	if (kind.startsWith("client.presence.") || kind === "client.remote_grant_revoked") updateClientPresence().catch((error) => addActivity("client.presence.refresh.error", { error: error.message }));
	if (kind.startsWith("task.") || kind.startsWith("agent.")) refreshTeam().catch((error) => addActivity("task.refresh.error", { error: error.message }));
	if (envelope.session_id && state.session && envelope.session_id !== state.session.id) return;
	if (kind === "turn.created" && envelope.turn_id) {
		state.turn = envelope.turn_id;
		setTurnRunning(true);
		if (payload.source_input_id) renderServerStartedInput(payload.source_input_id, envelope.item_id || payload.item_id, envelope.turn_id, envelope.session_id);
	}
	if (kind === "plan.updated") renderPlan(envelope.item_id || payload.item_id, envelope.turn_id, payload.title, Array.isArray(payload.steps) ? payload.steps : [], payload.status || envelope.status);
	if (kind === "question.required") {
		renderQuestion({
			id: envelope.request_id || payload.request_id,
			item_id: envelope.item_id || payload.item_id,
			turn_id: envelope.turn_id,
			questions: Array.isArray(payload.questions) ? payload.questions : [],
			allow_other: payload.allow_other !== false,
			expires_at: payload.expires_at || null,
			status: "pending",
			answers: []
		}, envelope.item_id || payload.item_id, envelope.turn_id);
		$("turn-state").textContent = "awaiting input";
		setTurnRunning(true);
	}
	if (kind === "question.answered") {
		markQuestionAnswered(envelope.request_id || payload.request_id, envelope.item_id || payload.item_id);
		$("turn-state").textContent = "continuing";
	}
	if (kind === "artifact.created") renderArtifact(envelope.item_id || payload.item_id, envelope.turn_id, payload.artifact_id, payload.title, payload.media_type);
	if (kind === "context.compacted") {
		const omitted = Number(payload.omitted_messages || 0);
		const truncated = Number(payload.truncated_messages || 0);
		const tokens = Number(payload.estimated_tokens || 0);
		const detail = [
			`${omitted} earlier message${omitted === 1 ? "" : "s"} summarized`,
			truncated ? `${truncated} long message${truncated === 1 ? "" : "s"} shortened` : "",
			tokens ? `about ${tokens.toLocaleString()} summary tokens` : ""
		].filter(Boolean).join(" · ");
		renderTranscriptNotice(envelope.item_id || payload.item_id || envelope.id, envelope.turn_id, "context_compaction", "Context optimized", detail);
	}
	if (kind === "model.rerouted") {
		const route = payload.from_model ? `${payload.from_model} → ${payload.to_model}` : String(payload.to_model || "fallback model");
		renderTranscriptNotice(envelope.item_id || envelope.id, envelope.turn_id, "model_reroute", "Model switched", `${route} · ${payload.reason || "routing policy"}`);
	}
	if (kind === "reasoning.summary.delta") {
		const itemId = envelope.item_id || payload.item_id || envelope.id;
		const prior = state.itemsById.get(itemId)?.querySelector("span")?.textContent || "";
		renderTranscriptNotice(itemId, envelope.turn_id, "reasoning_summary", "Reasoning summary", `${prior}${String(payload.text || "")}`);
	}
	if (kind === "turn.usage") {
		const inputTokens = Number(payload.input_tokens ?? payload.input_units ?? 0);
		const outputTokens = Number(payload.output_tokens ?? payload.output_units ?? 0);
		const modelCalls = Number(payload.model_calls || 0);
		const toolCalls = Number(payload.tool_calls || 0);
		state.usage.input_tokens += inputTokens;
		state.usage.output_tokens += outputTokens;
		state.usage.total_tokens = state.usage.input_tokens + state.usage.output_tokens;
		state.usage.model_calls += modelCalls;
		state.usage.tool_calls += toolCalls;
		if (envelope.turn_id && !state.usageTurns.has(envelope.turn_id)) {
			state.usageTurns.add(envelope.turn_id);
			state.usage.turns += 1;
		}
		renderTranscriptNotice(envelope.item_id || payload.item_id || envelope.id, envelope.turn_id, "usage", "Usage", `${(inputTokens + outputTokens).toLocaleString()} tokens · ${inputTokens.toLocaleString()} input + ${outputTokens.toLocaleString()} output · ${modelCalls} model / ${toolCalls} tool calls · ${String(payload.model || "model")}`);
	}
	if (kind === "agent.status") renderTranscriptNotice(envelope.item_id || payload.item_id || envelope.id, envelope.turn_id, "agent_status", "Agent", String(payload.label || payload.status || "updated"));
	if (kind.startsWith("hook.")) {
		const detail = [
			`${String(payload.event || "hook")} · ${String(payload.handler || "handler")}`,
			payload.input_modified ? "input modified" : "",
			payload.result_summary || payload.error_code || ""
		].filter(Boolean).join(" · ");
		renderTranscriptNotice(envelope.item_id || payload.item_id || envelope.id, envelope.turn_id, "hook", "Hook", detail);
	}
	if (kind === "model.delta") {
		const itemId = envelope.item_id || payload.item_id || null;
		let current = itemId ? state.itemsById.get(itemId) : null;
		if (!current?.classList.contains("streaming")) current = null;
		if (!current) {
			current = document.createElement("article");
			current.className = "message assistant streaming";
			if (itemId) current.dataset.itemId = itemId;
			if (envelope.turn_id) current.dataset.turnId = envelope.turn_id;
			current.setAttribute("aria-label", `${state.assistantAlias} response`);
			const label = document.createElement("div");
			label.className = "message-label";
			label.textContent = state.assistantAlias;
			const body = document.createElement("div");
			body.className = "message-body";
			current.append(label, body);
			current.dataset.raw = "";
			$("messages").append(current);
			if (itemId) state.itemsById.set(itemId, current);
			updateConversationState(true);
		}
		current.dataset.raw = `${current.dataset.raw ?? ""}${String(payload.text ?? "")}`;
		renderMessageContent(current, current.dataset.raw);
		advanceTranscript();
	}
	if ([
		"turn.created",
		"turn.status",
		"turn.awaiting_input",
		"turn.awaiting_approval",
		"turn.completed",
		"turn.failed",
		"turn.cancelled"
	].includes(kind)) $("turn-state").textContent = payload.status || kind.slice(5);
	if (kind === "turn.completed" || kind === "turn.failed" || kind === "turn.cancelled") {
		const itemId = envelope.item_id || payload.item_id || null;
		const current = itemId ? state.itemsById.get(itemId) : $("messages").querySelector(`.streaming[data-turn-id="${CSS.escape(envelope.turn_id || "")}"]`);
		if (current) {
			current.classList.remove("streaming");
			renderMessageContent(current, current.dataset.raw || "");
			delete current.dataset.raw;
		}
		if (kind === "turn.failed") renderMessage("assistant", `Agent failed: ${payload.error_code || "unknown error"}. Check daemon logs for the provider-safe diagnostic.`);
		if (envelope.turn_id === state.turn) {
			setTurnRunning(false);
			$("undo-turn").disabled = !state.capabilities.has("turn.undo");
		}
	}
	if (kind.startsWith("turn.input.")) refreshPendingInputs().catch((error) => addActivity("turn.input.refresh.error", { error: error.message }));
	if (kind === "session.goal.changed" && envelope.session_id === state.session?.id) loadSessionGoal(envelope.session_id).catch((error) => addActivity("session.goal.refresh.error", { error: error.message }));
	if (kind === "session.preferences.updated" && envelope.session_id === state.session?.id) loadSessionPreferences(envelope.session_id).catch((error) => addActivity("session.preferences.refresh.error", { error: error.message }));
	if (kind === "approval.required") renderApproval(payload.approval_id, payload.display || payload.tool, envelope.turn_id);
	if (kind.startsWith("team.")) refreshTeam();
}
function handleClientEvent(kind, value) {
	const envelope = parseClientEvent(value);
	const reduction = reduceClientEvent(transcriptProjection, envelope);
	transcriptProjection = reduction.state;
	state.after = Math.max(state.after, transcriptProjection.cursor);
	if (reduction.gap) throw new Error(`event sequence gap: expected ${reduction.gap.expected}, received ${reduction.gap.received}`);
	if (reduction.appendGap) throw new Error(`transcript append gap for ${reduction.appendGap.itemId}: expected byte offset ${reduction.appendGap.expected}, received ${reduction.appendGap.received}`);
	if (!reduction.accepted) return;
	if (!reduction.visible && !kind.startsWith("session.") && !kind.startsWith("terminal.") && !kind.startsWith("client.")) return;
	const notification = envelope.notification;
	if (!notification || typeof notification.type !== "string") {
		handleEvent(kind, isJsonObject(envelope.payload) ? envelope.payload : {}, envelope);
		return;
	}
	const notificationItemId = "item_id" in notification ? notification.item_id : null;
	const notificationRequestId = "request_id" in notification ? notification.request_id : null;
	const eventPayload = isJsonObject(envelope.payload) ? envelope.payload : {};
	const typed = {
		...envelope,
		item_id: notificationItemId || envelope.item_id,
		request_id: notificationRequestId || envelope.request_id
	};
	switch (notification.type) {
		case "agent_message_delta":
			handleEvent("model.delta", {
				text: notification.delta,
				item_id: notification.item_id,
				byte_offset: notification.byte_offset
			}, typed);
			break;
		case "turn_status_changed":
			handleEvent(kind, {
				status: notification.status,
				error_code: notification.error_code,
				item_id: eventPayload.item_id,
				source_input_id: eventPayload.source_input_id
			}, typed);
			break;
		case "tool_call_changed":
			handleEvent(kind, {
				tool_call_id: notification.item_id,
				model_call_id: notification.model_call_id,
				tool: notification.tool,
				display: notification.display,
				status: notification.status
			}, typed);
			break;
		case "approval_requested":
			handleEvent("approval.required", {
				approval_id: notification.request_id,
				tool_call_id: notification.tool_item_id,
				model_call_id: notification.model_call_id,
				tool: notification.tool,
				display: notification.summary
			}, typed);
			break;
		case "question_requested":
			handleEvent("question.required", {
				request_id: notification.request_id,
				item_id: notification.item_id,
				questions: notification.questions,
				allow_other: eventPayload.allow_other !== false
			}, typed);
			break;
		case "artifact_created":
			handleEvent("artifact.created", {
				item_id: notification.item_id,
				artifact_id: notification.artifact_id,
				title: notification.title,
				media_type: notification.media_type
			}, typed);
			break;
		case "turn_input_changed":
			handleEvent(kind, {
				input_id: notification.input_id,
				target_turn_id: notification.target_turn_id,
				resulting_turn_id: notification.resulting_turn_id,
				mode: notification.mode,
				status: notification.status
			}, typed);
			break;
		case "plan_updated":
			handleEvent("plan.updated", {
				item_id: notification.item_id,
				title: notification.title,
				steps: notification.steps,
				status: envelope.status
			}, typed);
			break;
		case "context_compacted":
			handleEvent("context.compacted", {
				item_id: notification.item_id,
				omitted_messages: notification.omitted_messages,
				truncated_messages: notification.truncated_messages,
				estimated_tokens: notification.estimated_tokens
			}, typed);
			break;
		case "model_rerouted":
			handleEvent("model.rerouted", {
				from_model: notification.from_model,
				to_model: notification.to_model,
				reason: notification.reason
			}, typed);
			break;
		case "reasoning_summary_delta":
			handleEvent("reasoning.summary.delta", {
				item_id: notification.item_id,
				text: notification.delta
			}, typed);
			break;
		case "mcp_progress_changed":
			handleEvent("mcp.progress", {
				item_id: notification.item_id,
				tool_call_id: notification.item_id,
				server: notification.server,
				tool: notification.tool,
				progress: notification.progress,
				total: notification.total,
				message: notification.message
			}, typed);
			break;
		case "usage_recorded":
			handleEvent("turn.usage", {
				item_id: notification.item_id,
				model: notification.model,
				input_tokens: notification.input_tokens,
				output_tokens: notification.output_tokens,
				total_tokens: notification.total_tokens,
				model_calls: notification.model_calls,
				tool_calls: notification.tool_calls
			}, typed);
			break;
		case "durable_task_changed":
			handleEvent(kind, {
				durable_task_id: notification.task_id,
				status: notification.status,
				attempt: notification.attempt,
				consumed_cost_micros: notification.consumed_cost_micros,
				consumed_runner_cost_micros: notification.consumed_runner_cost_micros
			}, typed);
			break;
		case "agent_run_changed":
			handleEvent("agent.status", {
				item_id: notification.item_id,
				agent_id: notification.agent_id,
				parent_agent_id: notification.parent_agent_id,
				status: notification.status,
				label: notification.label,
				attempt: notification.attempt,
				consumed_cost_micros: notification.consumed_cost_micros,
				consumed_runner_cost_micros: notification.consumed_runner_cost_micros
			}, typed);
			break;
		case "hook_changed":
			handleEvent(kind, {
				...eventPayload,
				item_id: notification.item_id,
				event: notification.event,
				handler: notification.handler,
				status: notification.status,
				input_modified: notification.input_modified
			}, typed);
			break;
		case "background_terminal_changed":
			handleEvent(kind, {
				terminal_id: notification.terminal_id,
				status: notification.status,
				output_byte_length: notification.output_byte_length,
				output_truncated: notification.output_truncated,
				exit_code: notification.exit_code,
				artifact_id: notification.artifact_id,
				revision: notification.revision
			}, typed);
			break;
		default: handleEvent(kind, eventPayload, envelope);
	}
}
async function subscribe(generation = state.generation) {
	if (generation !== state.generation) return;
	if (state.abort) state.abort.abort();
	state.abort = new AbortController();
	const controller = state.abort;
	const team = scope().team_id;
	try {
		const s = scope();
		const query = new URLSearchParams({
			organization_id: s.organization_id,
			team_id: team,
			actor_id: s.actor_id,
			after: String(state.after)
		});
		const response = await fetch(`/v1/events?${query}`, {
			credentials: "same-origin",
			signal: controller.signal,
			cache: "no-store"
		});
		if (!response.ok || !response.body) throw new Error(`events ${response.status}`);
		if (generation !== state.generation) {
			controller.abort();
			return;
		}
		if (!state.connected) {
			const capabilities = negotiateCapabilities(await api("/v1/capabilities", {
				allowDisconnected: true,
				signal: controller.signal
			}));
			if (generation !== state.generation) {
				controller.abort();
				return;
			}
			state.capabilities = capabilities;
			state.authenticatedScope = formScope();
			setConnection(true);
			await Promise.all([refreshSessions(), refreshTeam()]);
			await restoreRoute();
		}
		const reader = response.body.getReader();
		const decoder = new TextDecoder();
		let buffer = "";
		while (true) {
			const { value, done } = await reader.read();
			if (done) throw new Error("event stream closed");
			if (generation !== state.generation) {
				await reader.cancel();
				return;
			}
			buffer += decoder.decode(value, { stream: true });
			let boundary;
			while ((boundary = buffer.indexOf("\n\n")) >= 0) {
				const block = buffer.slice(0, boundary);
				buffer = buffer.slice(boundary + 2);
				let kind = "message", data = "";
				block.split("\n").forEach((line) => {
					if (line.startsWith("event:")) kind = line.slice(6).trim();
					if (line.startsWith("data:")) data += line.slice(5).trim();
				});
				if (data && generation === state.generation) handleClientEvent(kind, JSON.parse(data));
			}
		}
	} catch (error) {
		if (error.name !== "AbortError" && generation === state.generation) {
			setConnection(false, "reconnecting");
			const retryGeneration = state.generation;
			state.reconnectTimer = setTimeout(() => {
				state.reconnectTimer = null;
				if (retryGeneration === state.generation) subscribe(retryGeneration);
			}, 1200);
		}
	}
}
loadSettings();
applyTheme();
updateNotificationControls();
var savedPermissionMode = sessionStorage.getItem("oc.permission-mode");
if (savedPermissionMode === "accept_edits" || savedPermissionMode === "plan") state.permissionMode = savedPermissionMode;
updateContextChips();
updateConversationState(false);
if (sessionStorage.getItem("oc.sidebar-collapsed") === "true") document.body.classList.add("sidebar-collapsed");
var legacyDraft = sessionStorage.getItem("oc.prompt-draft");
if (legacyDraft && !sessionStorage.getItem("oc.prompt-draft:new")) sessionStorage.setItem("oc.prompt-draft:new", legacyDraft);
sessionStorage.removeItem("oc.prompt-draft");
restoreComposerDraft();
fields.forEach((id) => $(id).addEventListener("input", () => {
	settingsDirty = true;
	updateContextChips();
}));
identityFields.forEach((id) => $(id).addEventListener("input", () => {
	const transportActive = state.abort && !state.abort.signal.aborted;
	if (!state.connected && !state.connecting && !state.reconnectTimer && !transportActive) return;
	if (state.reconnectTimer) {
		clearTimeout(state.reconnectTimer);
		state.reconnectTimer = null;
	}
	if (state.abort) state.abort.abort();
	setConnection(false);
}));
$("prompt").addEventListener("input", () => {
	const value = $("prompt").value;
	if (value) sessionStorage.setItem(composerTextDraftKey(), value);
	else sessionStorage.removeItem(composerTextDraftKey());
	resizePrompt();
	if ($("prompt").value === "/") {
		$("prompt").value = "";
		sessionStorage.removeItem(composerTextDraftKey());
		resizePrompt();
		openCommands();
	}
	scheduleFileMentions();
	updateSendAction();
});
$("prompt").addEventListener("paste", (event) => {
	const files = event.clipboardData?.files;
	if (!files?.length) return;
	event.preventDefault();
	addDraftFiles(files);
	announce(`${files.length} pasted file${files.length === 1 ? "" : "s"} attached`);
});
$("prompt").addEventListener("keydown", (event) => {
	if (!$("mention-menu").hidden) {
		if (event.key === "ArrowDown" || event.key === "ArrowUp") {
			event.preventDefault();
			updateMentionSelection(mentionSelection + (event.key === "ArrowDown" ? 1 : -1));
			return;
		}
		if (event.key === "Enter" && !event.shiftKey) {
			event.preventDefault();
			const selected = $("mention-menu").querySelectorAll("button")[mentionSelection];
			if (selected) selectMention(selected.textContent);
			return;
		}
		if (event.key === "Escape") {
			event.preventDefault();
			closeMentionMenu();
			return;
		}
	}
	if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
		event.preventDefault();
		$("prompt-form").requestSubmit();
	}
});
$("connection-form").addEventListener("submit", (event) => {
	event.preventDefault();
	connect();
});
$("notification-mode").addEventListener("change", () => {
	const mode = $("notification-mode").value;
	localStorage.setItem("oc.notification-mode", mode === "background" || mode === "always" ? mode : "off");
	updateNotificationControls();
});
$("enable-notifications").addEventListener("click", () => {
	enableNotifications().catch((error) => toast(error.message));
});
$("refresh").addEventListener("click", () => refreshSessions().catch((error) => addActivity("session.refresh.error", { error: error.message })));
$("session-search").addEventListener("input", () => {
	sessionWindowStart = 0;
	renderSessions();
});
$("session-filter").addEventListener("change", () => {
	sessionWindowStart = 0;
	renderSessions();
});
$("messages").addEventListener("scroll", () => {
	transcriptFollowing = transcriptIsNearBottom();
	if (transcriptFollowing) $("jump-latest").hidden = true;
});
$("jump-latest").addEventListener("click", () => advanceTranscript(true));
$("load-earlier").addEventListener("click", () => {
	loadEarlierTranscript().catch((error) => toast(error.message));
});
$("refresh-team").addEventListener("click", refreshTeam);
$("new-goal").addEventListener("click", () => createGoal().catch((error) => addActivity("goal.error", { error: error.message })));
$("new-task").addEventListener("click", () => createTask().catch((error) => addActivity("task.error", { error: error.message })));
$("set-capacity").addEventListener("click", () => setCapacity().catch((error) => addActivity("capacity.error", { error: error.message })));
$("new-ownership").addEventListener("click", () => createOwnership().catch((error) => addActivity("ownership.error", { error: error.message })));
$("new-budget").addEventListener("click", () => createBudget().catch((error) => addActivity("budget.error", { error: error.message })));
$("new-background-task").addEventListener("click", () => createBackgroundTask().catch((error) => addActivity("task.background.error", { error: error.message })));
$("new-background-terminal").addEventListener("click", () => createBackgroundTerminal().catch((error) => addActivity("terminal.background.error", { error: error.message })));
$("create-session").addEventListener("click", createSession);
$("prompt-form").addEventListener("submit", runTurn);
$("steer-turn").addEventListener("click", async () => {
	const content = $("prompt").value.trim();
	if (!content || !state.turnRunning || composerSubmissionPending) return;
	await withComposerSubmission(async () => {
		try {
			await submitTurnInput(content, "steer");
		} catch (error) {
			addActivity("turn.input.steer.error", { error: error.message });
		}
	});
});
$("show-diff").addEventListener("click", () => showDiff().catch((error) => toast(error.message)));
$("show-context").addEventListener("click", showContext);
$("review-session").addEventListener("click", startReview);
$("quick-diff").addEventListener("click", () => {
	openDrawer("inspector");
	showDiff().catch((error) => toast(error.message));
});
$("undo-turn").addEventListener("click", () => undoTurn());
$("show-checkpoints").addEventListener("click", showCheckpoints);
$("fork-session").addEventListener("click", forkSession);
$("promote-side-conversation").addEventListener("click", () => {
	promoteSideConversation().catch((error) => toast(error.message));
});
$("close-side-conversation").addEventListener("click", () => {
	closeSideConversation().catch((error) => toast(error.message));
});
$("show-branches").addEventListener("click", showBranches);
$("export-session").addEventListener("click", exportSession);
$("rename-session").addEventListener("click", renameSession);
$("rename-assistant").addEventListener("click", () => {
	renameAssistant().catch((error) => toast(error.message));
});
$("edit-session-goal").addEventListener("click", () => {
	editSessionGoal().catch((error) => toast(error.message));
});
$("toggle-session-goal").addEventListener("click", () => {
	toggleSessionGoal().catch((error) => toast(error.message));
});
$("clear-session-goal").addEventListener("click", () => {
	clearSessionGoal().catch((error) => toast(error.message));
});
$("cancel-session").addEventListener("click", () => closeSession(false));
$("delete-session").addEventListener("click", () => closeSession(true));
$("new-chat").addEventListener("click", () => {
	clearSessionSelection();
	document.body.classList.remove("mobile-sidebar-open");
});
$("toggle-sidebar").addEventListener("click", toggleHistory);
$("open-sidebar").addEventListener("click", toggleHistory);
$("sidebar-scrim").addEventListener("click", () => document.body.classList.remove("mobile-sidebar-open"));
$("toggle-inspector").addEventListener("click", () => $("inspector").classList.contains("open") ? closeDrawers() : openDrawer("inspector"));
$("close-inspector").addEventListener("click", () => closeDrawers());
$("open-settings").addEventListener("click", () => openDrawer("settings-drawer"));
$("open-projects").addEventListener("click", () => showProjects());
$("open-artifacts").addEventListener("click", () => showArtifacts());
$("open-extensions").addEventListener("click", () => showExtensions());
$("add-mcp-server").addEventListener("click", () => {
	addMcpServer().catch((error) => toast(error.message));
});
$("add-skill").addEventListener("click", () => {
	addSkill().catch((error) => toast(error.message));
});
$("add-hook").addEventListener("click", () => {
	addHook().catch((error) => toast(error.message));
});
$("manage-marketplaces").addEventListener("click", () => {
	managePluginMarketplaces().catch((error) => toast(error.message));
});
$("refresh-extensions").addEventListener("click", () => {
	loadExtensionCatalog().catch((error) => toast(error.message));
});
$("extension-filter").addEventListener("change", renderExtensionList);
$("extension-search").addEventListener("input", renderExtensionList);
$("refresh-artifacts").addEventListener("click", () => {
	const route = parseRoute(window.location.pathname);
	loadArtifactPage(true, route.type === "artifact" ? route.artifactId : null).catch((error) => toast(error.message));
});
$("artifact-filter").addEventListener("change", renderArtifactList);
$("load-more-artifacts").addEventListener("click", () => {
	loadMoreArtifacts().catch((error) => toast(error.message));
});
$("composer-settings").addEventListener("click", () => {
	if (state.turnRunning) {
		toast("Wait for the current Turn to finish before attaching files.");
		return;
	}
	$("attachment-input").click();
});
$("attachment-input").addEventListener("change", () => {
	const files = $("attachment-input").files;
	if (files) addDraftFiles(files);
	$("attachment-input").value = "";
});
$("workspace-chip").addEventListener("click", () => openDrawer("settings-drawer"));
$("model-chip").addEventListener("click", () => openModelPicker().catch((error) => toast(error.message)));
$("empty-connect").addEventListener("click", () => openDrawer("settings-drawer"));
$("retry-connection").addEventListener("click", connect);
$("offline-diagnostics").addEventListener("click", () => openDrawer("settings-drawer"));
$("close-settings").addEventListener("click", () => closeDrawers());
$("drawer-scrim").addEventListener("click", () => closeDrawers());
$("open-team").addEventListener("click", () => showTeam());
$("projects-new-task").addEventListener("click", () => {
	clearSessionSelection();
	showWorkspace();
});
$("team-new-task-primary").addEventListener("click", () => createTask().catch((error) => addActivity("task.error", { error: error.message })));
$("audit-filter-form").addEventListener("submit", (event) => {
	event.preventDefault();
	refreshTeam().catch((error) => toast(error.message));
});
$("clear-audit-filters").addEventListener("click", () => {
	[
		"audit-filter-actor",
		"audit-filter-action",
		"audit-filter-session",
		"audit-filter-since",
		"audit-filter-until"
	].forEach((id) => {
		$(id).value = "";
	});
	refreshTeam().catch((error) => toast(error.message));
});
$("export-team-audit").addEventListener("click", () => {
	exportTeamAudit().catch((error) => toast(error.message));
});
$("theme-toggle").addEventListener("click", cycleTheme);
$("open-diagnostics").addEventListener("click", () => {
	closeUserMenu();
	openDrawer("settings-drawer");
	$("connection").scrollIntoView({ block: "center" });
});
$("permission-chip").addEventListener("click", () => openPermissionPicker().catch((error) => toast(error.message)));
$("close-context-picker").addEventListener("click", () => $("context-picker-dialog").close());
$("context-picker-query").addEventListener("input", () => {
	contextPickerSelection = 0;
	renderContextPicker();
});
$("context-picker-query").addEventListener("keydown", (event) => {
	const options = renderContextPicker();
	if ((event.key === "ArrowDown" || event.key === "ArrowUp") && options.length) {
		event.preventDefault();
		const direction = event.key === "ArrowDown" ? 1 : -1;
		contextPickerSelection = (contextPickerSelection + direction + options.length) % options.length;
		renderContextPicker();
		document.getElementById(`context-picker-option-${contextPickerSelection}`)?.scrollIntoView({ block: "nearest" });
	} else if (event.key === "Enter" && options[contextPickerSelection]) {
		event.preventDefault();
		const option = options[contextPickerSelection];
		if (!option.disabled) Promise.resolve(option.select()).then(() => $("context-picker-dialog").close()).catch((error) => toast(error.message));
	}
});
$("open-command").addEventListener("click", openCommands);
$("open-shortcuts-inline").addEventListener("click", showShortcuts);
$("command-query").addEventListener("input", () => {
	commandSelection = 0;
	renderCommands();
});
$("command-query").addEventListener("keydown", (event) => {
	const commands = renderCommands();
	if ((event.key === "ArrowDown" || event.key === "ArrowUp") && commands.length) {
		event.preventDefault();
		const direction = event.key === "ArrowDown" ? 1 : -1;
		commandSelection = (commandSelection + direction + commands.length) % commands.length;
		renderCommands();
		$(`command-option-${commandSelection}`)?.scrollIntoView({ block: "nearest" });
	} else if (event.key === "Enter" && commands[commandSelection]) {
		event.preventDefault();
		const command = commands[commandSelection];
		if (!command.enabled || command.enabled()) runCommand(command);
	}
});
$("action-form").addEventListener("submit", submitActionDialog);
$("close-action").addEventListener("click", () => closeActionDialog(null));
$("cancel-action").addEventListener("click", () => closeActionDialog(null));
$("action-dialog").addEventListener("cancel", (event) => {
	event.preventDefault();
	closeActionDialog(null);
});
document.querySelector(".dialog-close")?.addEventListener("click", () => $("shortcuts-dialog").close());
$("shortcuts-dialog").addEventListener("keydown", (event) => {
	if (event.key === "Tab") {
		event.preventDefault();
		document.querySelector(".dialog-close")?.focus();
	}
});
document.querySelectorAll(".starter-actions button").forEach((button) => button.addEventListener("click", () => {
	$("prompt").value = button.dataset.prompt || "";
	sessionStorage.setItem(composerTextDraftKey(), $("prompt").value);
	resizePrompt();
	$("prompt").focus();
	announce("Suggested task added. Edit it or press Enter to run.");
}));
document.querySelector(".brand")?.addEventListener("click", (event) => {
	event.preventDefault();
	clearSessionSelection();
});
document.querySelectorAll(".team-tabs button").forEach((button) => button.addEventListener("click", () => {
	const section = button.dataset.teamSection;
	if (!section) return;
	selectTeamSection(section, "smooth");
	routePath(teamRoute(section));
}));
document.addEventListener("keydown", (event) => {
	const commandKey = event.metaKey || event.ctrlKey;
	if (commandKey && event.key.toLowerCase() === "k") {
		event.preventDefault();
		openCommands();
		return;
	}
	if (commandKey && event.key.toLowerCase() === "n") {
		event.preventDefault();
		clearSessionSelection();
		return;
	}
	if (commandKey && event.key.toLowerCase() === "l") {
		event.preventDefault();
		$("prompt").focus();
		return;
	}
	if (commandKey && event.key.toLowerCase() === "d") {
		event.preventDefault();
		if (state.session) {
			openDrawer("inspector");
			showDiff().catch((error) => toast(error.message));
		}
		return;
	}
	if (commandKey && event.key.toLowerCase() === "b") {
		event.preventDefault();
		toggleHistory();
		return;
	}
	if (commandKey && event.key.toLowerCase() === "f") {
		event.preventDefault();
		focusSessionSearch();
		return;
	}
	if (commandKey && event.key === "/") {
		event.preventDefault();
		showShortcuts();
		return;
	}
	if (event.key === "Escape") {
		closeDrawers();
		document.body.classList.remove("mobile-sidebar-open");
	}
});
window.addEventListener("popstate", () => {
	restoreRoute().catch((error) => toast(error.message));
});
["focus", "blur"].forEach((eventName) => window.addEventListener(eventName, () => {
	updateClientPresence().catch(() => {});
}));
document.addEventListener("visibilitychange", () => {
	updateClientPresence().catch(() => {});
});
presenceTimer = window.setInterval(() => {
	updateClientPresence().catch(() => {});
}, 15e3);
window.addEventListener("pagehide", () => {
	if (state.reconnectTimer) clearTimeout(state.reconnectTimer);
	if (developmentReloadTimer) clearInterval(developmentReloadTimer);
	if (presenceTimer) clearInterval(presenceTimer);
	if (state.abort) state.abort.abort();
	setConnection(false);
});
enableDevelopmentAutoReload();
bootstrapBrowserSession().then((ready) => {
	if (ready) connect();
}).catch((error) => setConnection(false, error.message));
resizePrompt();
restoreRoute().catch((error) => toast(error.message));
//#endregion
