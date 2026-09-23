import { reactive } from 'vue';

/**
 * Messages written while the agent is still working.
 *
 * Neither client accepts a second message mid-turn, so pressing Enter then
 * used to do nothing. The message waits here instead, visible above the
 * composer where it can still be edited back or removed, and the pane sends
 * the first one the moment the turn ends. Held per session in memory only: a
 * reload drops the queue rather than sending something the user no longer sees.
 */
export interface QueuedMessage { id: string; text: string }

const queues = reactive(new Map<string, QueuedMessage[]>());

export const messageQueue = {
  get(sessionId: string): QueuedMessage[] {
    if (!queues.has(sessionId)) queues.set(sessionId, []);
    return queues.get(sessionId)!;
  },
  push(sessionId: string, message: QueuedMessage) { messageQueue.get(sessionId).push(message); },
  remove(sessionId: string, id: string) {
    const list = messageQueue.get(sessionId), index = list.findIndex(item => item.id === id);
    return index < 0 ? undefined : list.splice(index, 1)[0];
  },
  shift(sessionId: string) { return messageQueue.get(sessionId).shift(); },
};
