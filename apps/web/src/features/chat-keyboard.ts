export interface ChatKeyEvent {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  isComposing?: boolean;
  keyCode?: number;
  repeat?: boolean;
  defaultPrevented?: boolean;
}

export function isComposingKey(event: ChatKeyEvent, composing = false): boolean {
  return composing || event.isComposing === true || event.keyCode === 229;
}

export function composerKeyAction(event: ChatKeyEvent, composing = false): 'send' | 'newline' | 'suppress' | 'none' {
  if (event.defaultPrevented || isComposingKey(event, composing) || event.key !== 'Enter') return 'none';
  if (event.shiftKey || event.altKey) return 'newline';
  return event.repeat ? 'suppress' : 'send';
}

export function chatEscapeAction(state: {
  menuOpen: boolean;
  confirmEnd: boolean;
  confirmEndpoint: boolean;
  actionBusy: boolean;
  composerFocused: boolean;
  turnRunning: boolean;
  awaitingApproval: boolean;
  preview: boolean;
}): 'close-menu' | 'cancel-end' | 'cancel-endpoint' | 'interrupt' | 'blur' | 'none' {
  if (state.menuOpen) return 'close-menu';
  if (state.actionBusy) return 'none';
  if (state.confirmEnd) return 'cancel-end';
  if (state.confirmEndpoint) return 'cancel-endpoint';
  if (!state.composerFocused) return 'none';
  if (!state.preview && state.turnRunning && !state.awaitingApproval) return 'interrupt';
  return 'blur';
}
