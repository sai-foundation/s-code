import type { TranscriptItem } from "../models/protocol";
import type { TranscriptItemContent } from "../../generated/protocol/TranscriptItemContent";

type MessageTranscriptItem = TranscriptItem & {
  content: Extract<TranscriptItemContent, { type: "message" }>;
};

type ToolTranscriptItem = TranscriptItem & {
  content: Extract<TranscriptItemContent, { type: "tool_call" | "mcp_call" }>;
};

export function transcriptItemIdentity(item: TranscriptItem): {
  itemId: string;
  turnId: string;
} {
  return { itemId: item.id, turnId: item.turn_id };
}

export function isMessageItem(item: TranscriptItem): item is MessageTranscriptItem {
  return (
    (item.kind === "user_message" || item.kind === "agent_message")
    && item.content.type === "message"
  );
}

export function isToolItem(item: TranscriptItem): item is ToolTranscriptItem {
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
