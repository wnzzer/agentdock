/**
 * Translating what a client says about its own models.
 *
 * These lines are written by Claude Code and Codex rather than by AgentDock:
 * they name models, quote prices, and change with the client's version, so
 * there is no fixed set of them to put in a dictionary. What is fixed are the
 * phrases between the separators — "Efficient for routine tasks" is the same
 * sentence whichever model it describes — so each segment is translated on its
 * own and anything unrecognised passes through exactly as the client wrote it.
 *
 * A model's name is never touched. It is the thing's name.
 */
type Translate = (source: string, values?: Record<string, string>) => string;

/**
 * Segments that carry a value the client filled in. The value is lifted out so
 * the sentence around it can be translated, and put back untranslated.
 */
const FILLED: { pattern: RegExp; template: string; name: string }[] = [
  { pattern: /^Use the default model \(currently (.+)\)$/, template: 'Use the default model (currently {model})', name: 'model' },
  { pattern: /^(\$[\d.]+\/\$[\d.]+) per Mtok$/, template: '{price} per Mtok', name: 'price' },
];

export function describeModel(description: string, t: Translate): string {
  return description
    .split('·')
    .map(segment => segment.trim())
    .map(segment => translateSegment(segment, t))
    .join(' · ');
}

function translateSegment(segment: string, t: Translate): string {
  if (!segment) return segment;
  for (const { pattern, template, name } of FILLED) {
    const match = pattern.exec(segment);
    if (match) return t(template, { [name]: match[1] });
  }
  return t(segment);
}
