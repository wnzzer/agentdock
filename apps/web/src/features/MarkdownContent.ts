import { cloneVNode, defineComponent, h, reactive } from 'vue';
import { markdownBlocksWithLines, markdownInline, workspaceImagePath, type MarkdownInline } from './chat-model';
import { openLightbox } from './image-lightbox';
import { mediaKind } from './media-kind';
import CopyButton from './CopyButton.vue';

/**
 * Renders the markdown subset both the conversation and the file pane show.
 *
 * Shared rather than copied so a file and a message that contain the same text
 * render the same way — and so a fix to either reaches both. The `chat-`
 * classes are kept as the style contract; the file pane scopes its own spacing
 * around them instead of restyling each element.
 */
export const MarkdownContent = defineComponent({
  props: {
    text: { type: String, required: true },
    /** Turns a workspace path into a URL to load it from. Without one, an
     * image stays as its alt text: nothing is loaded that was not asked for. */
    imageUrl: { type: Function as unknown as () => (path: string) => string, required: false },
    /** Turns an absolute host path into a URL to load it from. Without one, a
     * leading slash means the workspace root, as it does in a repository's docs;
     * with one -- in a conversation, where agents write real paths -- it means
     * the host's file system. */
    hostImageUrl: { type: Function as unknown as () => (path: string) => string, required: false },
    /** Directory of the file being rendered, for relative image paths. */
    base: { type: String, default: '' },
    /** Opens a file a message points at. Without one, references render as
     * the text they were written as. */
    openFile: { type: Function as unknown as () => (reference: { path: string; line?: number }) => void, required: false },
    /** Marks each block with the source line it starts on (`data-line`), so an
     * editor beside it can keep the two aligned. */
    sourceLines: { type: Boolean, default: false },
  },
  setup(props) {
    /** Images that would not load: shown as a link to the file instead, which
     * can explain why -- outside the folders AgentDock may browse, say. */
    const failed = reactive(new Set<string>());
    const hostPath = (src: string) => {
      let value = src.trim().replace(/^file:\/\//i, '');
      try { value = decodeURIComponent(value); } catch { return undefined; }
      value = value.split(/[?#]/)[0];
      return /^(\/(?!\/)|[A-Za-z]:[\\/])/.test(value) && ['image', 'converted'].includes(mediaKind(value)) ? value : undefined;
    };
    const imageSource = (src: string): { url: string; path: string } | undefined => {
      const absolute = props.hostImageUrl && hostPath(src);
      if (absolute) return { url: props.hostImageUrl!(absolute), path: absolute };
      const path = props.imageUrl && workspaceImagePath(src, props.base);
      return path ? { url: props.imageUrl!(path), path } : undefined;
    };
    const inline = (text: string) =>
      markdownInline(text).map((part: MarkdownInline) =>
        part.type === 'file'
          ? (props.openFile
              ? (() => {
                  const slash = part.path.lastIndexOf('/'), name = part.path.slice(slash + 1), dir = slash > 0 ? part.path.slice(0, slash + 1) : '';
                  return h('button', {
                    type: 'button', class: ['chat-file-ref', { 'is-code': part.code }], title: part.path + (part.line ? `:${part.line}` : ''),
                    onClick: () => props.openFile!({ path: part.path, line: part.line }),
                  }, [
                    h('svg', { viewBox: '0 0 16 16', width: 11, height: 11, 'aria-hidden': 'true' }, [h('path', { d: 'M4 1.5h5l3 3v10H4zM9 1.5v3h3', fill: 'none', stroke: 'currentColor', 'stroke-width': 1.3, 'stroke-linejoin': 'round' })]),
                    dir ? h('span', { class: 'chat-file-dir' }, dir.length > 28 ? '…' + dir.slice(-27) : dir) : null,
                    h('span', { class: 'chat-file-name' }, name),
                    part.line ? h('span', { class: 'chat-file-line' }, `:${part.line}`) : null,
                  ]);
                })()
              : part.code ? h('code', part.text) : part.text)
          : part.type === 'image'
          ? (() => {
              const found = imageSource(part.src);
              if (!found) return part.alt || part.src;
              const { path } = found;
              const src = mediaKind(path) === 'converted' ? `${found.url}${found.url.includes('?') ? '&' : '?'}as=png` : found.url;
              if (failed.has(src)) {
                return props.openFile
                  ? h('button', { type: 'button', class: 'chat-file-ref', title: path, onClick: () => props.openFile!({ path }) }, [h('span', { class: 'chat-file-name' }, part.alt || path.split('/').pop())])
                  : part.alt || part.src;
              }
              return h('img', { class: 'chat-inline-image', src, alt: part.alt, title: part.alt || path, loading: 'lazy', onClick: () => openLightbox(src, part.alt || path), onError: () => failed.add(src) });
            })()
          : part.type === 'link'
          // Untrusted destinations: opening in a new tab without these lets the
          // opened page reach back through window.opener.
          ? h('a', { href: part.href, target: '_blank', rel: 'noopener noreferrer' }, part.text)
          : part.type === 'text'
            ? part.text
            : h(part.type === 'strong' ? 'strong' : part.type === 'em' ? 'em' : 'code', part.text));
    return () =>
      h('div', { class: 'chat-markdown' }, markdownBlocksWithLines(props.text).map(({ block, line }) => {
        const node =
        block.type === 'code'
          ? h('div', { class: 'chat-code' }, [block.language ? h('small', block.language) : null, h('div', { class: 'chat-code-copy' }, [h(CopyButton, { text: block.text, label: 'Copy code', compact: true })]), h('pre', [h('code', block.text)])])
          : block.type === 'table'
            // Wide tables scroll inside their own box rather than the message.
            ? h('div', { class: 'chat-table' }, [h('table', [
                h('thead', [h('tr', block.header.map((cell, index) => h('th', { style: block.align[index] ? { textAlign: block.align[index] } : undefined }, inline(cell))))]),
                h('tbody', block.rows.map(row => h('tr', row.map((cell, index) => h('td', { style: block.align[index] ? { textAlign: block.align[index] } : undefined }, inline(cell)))))),
              ])])
          : block.type === 'list'
            ? h(block.ordered ? 'ol' : 'ul', block.items.map(item => h('li', inline(item))))
            : h(block.type === 'heading' ? `h${Math.min(block.level ?? 3, 6)}` : block.type === 'quote' ? 'blockquote' : 'p', inline(block.text));
        return props.sourceLines ? cloneVNode(node, { 'data-line': line }) : node;
      }));
  },
});
