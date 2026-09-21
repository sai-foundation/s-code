/** Exact local command grammar shared with the daemon; never interprets prose. */
export function isProtectionCommand(content: string): boolean {
  const text = content.trim();
  return /^(?:\/protect|protect file)(?:\s|$)/u.test(text) || /^(?:保护文件|保护目录)/u.test(text);
}
export type ComposerRoute = "commands" | "protection" | "queue" | "cancel" | "send";
export function composerRoute(content: string, running: boolean): ComposerRoute {
  if (content.trim() === "/") return "commands";
  if (isProtectionCommand(content)) return "protection";
  return running ? content ? "queue" : "cancel" : "send";
}
/** The form and active-turn fallback use this routing before queue or model work. */
export async function dispatchComposer(content: string, running: boolean, handlers: Record<ComposerRoute, (content: string) => void | Promise<void>>) {
  await handlers[composerRoute(content, running)](content);
}
