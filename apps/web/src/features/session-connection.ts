import { createSessionConnections } from './session-connection-state';
import { json, request } from './api';
import type { Session } from '@agentdock/protocol';
export const sessionConnections=createSessionConnections(session=>request<Session>(`/sessions/${encodeURIComponent(session.id)}/start`,json('POST')));
