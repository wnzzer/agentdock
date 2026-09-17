import { defineComponent, h } from 'vue';
import { markdownBlocks, markdownInline, type MarkdownInline } from './chat-model';

/**
 * Renders the markdown subset both the conversation and the file pane show.
 *
 * Shared rather than copied so a file and a message that contain the same text
 * render the same way — and so a fix to either reaches both. The `chat-`
 * classes are kept as the style contract; the file pane scopes its own spacing
 * around them instead of restyling each element.
 */
export const MarkdownContent = defineComponent({
  props: { text: { type: String, required: true } },
  setup(props) {
    const inline = (text: string) =>
      markdownInline(text).map((part: MarkdownInline) =>
        part.type === 'link'
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
