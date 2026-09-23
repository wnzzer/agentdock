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
  },
  setup(props) {
    const inline = (text: string) =>
      markdownInline(text).map((part: MarkdownInline) =>
        part.type === 'image'
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
          : block.type === 'list'
            ? h(block.ordered ? 'ol' : 'ul', block.items.map(item => h('li', inline(item))))
            : h(block.type === 'heading' ? `h${Math.min(block.level ?? 3, 6)}` : block.type === 'quote' ? 'blockquote' : 'p', inline(block.text))));
  },
});
