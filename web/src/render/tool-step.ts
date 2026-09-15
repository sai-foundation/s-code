/** Only an unbound, top-level model proposal can become a real tool call. */
export function canBindToolProposal(
  candidate: { parentToolCallId?: string; proposed?: string; turnId?: string; tool?: string },
  turnId: string,
  tool: string,
): boolean {
  return !candidate.parentToolCallId && candidate.proposed === "true"
    && candidate.turnId === turnId && candidate.tool === tool;
}
