/**
 * Menus built on <details> stay open until their own summary is clicked
 * again; a press anywhere else left the pane's "+" menu, the session menu and
 * others hanging open. One listener closes every open menu the press did not
 * land in, so each menu does not need its own.
 *
 * Only menus are closed. Disclosures that hold content -- the environment
 * editor, a folded tool run -- are meant to stay open and are not listed.
 */
export const MENU_SELECTOR = 'details.dock-add-menu[open], details.dock-layout-menu[open], details.chat-menu[open], details.session-menu[open], details.chip-menu[open]';

export function closeMenusOutside(target: EventTarget | null, root: ParentNode = document) {
  const node = target instanceof Node ? target : null;
  for (const menu of root.querySelectorAll<HTMLDetailsElement>(MENU_SELECTOR)) {
    if (!node || !menu.contains(node)) menu.open = false;
  }
}

export function installMenuDismissal(doc: Document = document) {
  const listener = (event: Event) => closeMenusOutside(event.target, doc);
  doc.addEventListener('pointerdown', listener, true);
  const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') closeMenusOutside(null, doc); };
  doc.addEventListener('keydown', onKey);
  return () => { doc.removeEventListener('pointerdown', listener, true); doc.removeEventListener('keydown', onKey); };
}
