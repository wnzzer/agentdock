import { defineComponent, h } from 'vue';
import { markdownBlocks, markdownInline, workspaceImagePath, type MarkdownInline } from './chat-model';
import { openLightbox } from './image-lightbox';

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
    /** Directory of the file being rendered, for relative image paths. */
    base: { type: String, default: '' },
    /** Opens a file a message points at. Without one, references render as
     * the text they were written as. */
    openFile: { type: Function as unknown as () => (reference: { path: string; line?: number }) => void, required: false },
  },
  setup(props) {
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
              const path = props.imageUrl && workspaceImagePath(part.src, props.base);
              if (!path) return part.alt || part.src;
              const src = props.imageUrl!(path);
              return h('img', { class: 'chat-inline-image', src, alt: part.alt, title: part.alt || path, loading: 'lazy', onClick: () => openLightbox(src, part.alt || path) });
            })()
          : part.type === 'link'
          // Untrusted destinations: opening in a new tab without these lets the
          // opened page reach back through window.opener.
          ? h('a', { href: part.href, target: '_blank', rel: 'noopener noreferrer' }, part.text)
          : part.type === 'text'
            ? part.text
            : h(part.type === 'strong' ? 'strong' : part.type === 'em' ? 'em' : 'code', part.text));
    return () =>
      h('div', { class: 'chat-markdown' }, markdownBlocks(props.text).map(block =>
        block.type === 'code'
          ? h('div', { class: 'chat-code' }, [block.language ? h('small', block.language) : null, h('pre', [h('code', block.text)])])
          : block.type === 'table'
            // Wide tables scroll inside their own box rather than the message.
            ? h('div', { class: 'chat-table' }, [h('table', [
                h('thead', [h('tr', block.header.map((cell, index) => h('th', { style: block.align[index] ? { textAlign: block.align[index] } : undefined }, inline(cell))))]),
                h('tbody', block.rows.map(row => h('tr', row.map((cell, index) => h('td', { style: block.align[index] ? { textAlign: block.align[index] } : undefined }, inline(cell)))))),
              ])])
          : block.type === 'list'
            ? h(block.ordered ? 'ol' : 'ul', block.items.map(item => h('li', inline(item))))
            : h(block.type === 'heading' ? `h${Math.min(block.level ?? 3, 6)}` : block.type === 'quote' ? 'blockquote' : 'p', inline(block.text))));
  },
});
