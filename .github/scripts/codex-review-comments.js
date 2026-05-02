'use strict';

const CODEX_REVIEW_MARKER = '<!-- codex-review -->';

function normalizeReviewBody(rawBody) {
  const body = String(rawBody || '').trim();

  if (body) {
    return body;
  }

  return [
    '## Codex Review',
    '',
    'Codex review did not produce output. Check the workflow logs.',
  ].join('\n');
}

function changedRightLines(patch) {
  const lines = new Set();

  if (!patch) {
    return lines;
  }

  let rightLine = null;

  for (const line of patch.split('\n')) {
    const hunk = /^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) {
      rightLine = Number(hunk[1]);
      continue;
    }

    if (rightLine === null) {
      continue;
    }

    if (line.startsWith('+') && !line.startsWith('+++')) {
      lines.add(rightLine);
      rightLine += 1;
      continue;
    }

    if (line.startsWith(' ')) {
      lines.add(rightLine);
      rightLine += 1;
      continue;
    }

    if (line.startsWith('-') && !line.startsWith('---')) {
      continue;
    }
  }

  return lines;
}

function buildChangedLineMap(files) {
  const map = new Map();

  for (const file of files || []) {
    if (!file || !file.filename) {
      continue;
    }

    const lines = changedRightLines(file.patch || '');
    if (lines.size > 0) {
      map.set(file.filename, lines);
    }
  }

  return map;
}

function normalizeCommentPayload(payload, changedLineMap) {
  if (!payload || typeof payload !== 'object') {
    return null;
  }

  const path = String(payload.path || '').trim();
  const line = Number(payload.line);
  const body = String(payload.body || '').trim();

  if (!path || !Number.isInteger(line) || line < 1 || !body) {
    return null;
  }

  const changedLines = changedLineMap.get(path);
  if (!changedLines || !changedLines.has(line)) {
    return null;
  }

  return {
    path,
    line,
    side: 'RIGHT',
    body,
  };
}

function collectPayloads(value) {
  if (Array.isArray(value)) {
    return value;
  }

  if (value && Array.isArray(value.comments)) {
    return value.comments;
  }

  if (value && typeof value === 'object') {
    return [value];
  }

  return [];
}

function parseFencedPayloads(markdown, changedLineMap) {
  const comments = [];
  const fencePattern = /```codex-review-comments?\s*\n([\s\S]*?)```/g;
  let match;

  while ((match = fencePattern.exec(markdown)) !== null) {
    try {
      const parsed = JSON.parse(match[1]);
      for (const payload of collectPayloads(parsed)) {
        const comment = normalizeCommentPayload(payload, changedLineMap);
        if (comment) {
          comments.push(comment);
        }
      }
    } catch (_) {
      continue;
    }
  }

  return comments;
}

function parseHiddenPayloads(markdown, changedLineMap) {
  const comments = [];
  const blockPattern = /<!--\s*codex-review-comment\s*\n([\s\S]*?)-->/g;
  let match;

  while ((match = blockPattern.exec(markdown)) !== null) {
    const block = match[1];
    const path = /^path:\s*(.+)$/im.exec(block);
    const line = /^line:\s*(\d+)$/im.exec(block);
    const body = /^body:\s*\n([\s\S]*)$/im.exec(block);

    const comment = normalizeCommentPayload(
      {
        path: path ? path[1].trim() : '',
        line: line ? line[1] : '',
        body: body ? body[1].trim() : '',
      },
      changedLineMap,
    );

    if (comment) {
      comments.push(comment);
    }
  }

  return comments;
}

function splitFindingBlocks(markdown) {
  const blocks = [];
  let current = [];

  for (const line of markdown.split(/\r?\n/)) {
    const startsBlock =
      /^#{3,6}\s+\S/.test(line) ||
      /^\s*(?:[-*]|\d+[.)])\s+(?:\*\*)?(?:P[0-3]|critical|high|medium|low|blocking|finding|issue|bug)\b/i.test(line);

    if (startsBlock && current.some((entry) => entry.trim())) {
      blocks.push(current.join('\n').trim());
      current = [];
    }

    current.push(line);
  }

  if (current.some((entry) => entry.trim())) {
    blocks.push(current.join('\n').trim());
  }

  return blocks;
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function extractLine(block, path) {
  const escapedPath = escapeRegex(path);
  const patterns = [
    new RegExp(`${escapedPath}:(\\d+)`, 'i'),
    new RegExp(`${escapedPath}#L(\\d+)`, 'i'),
    new RegExp(`${escapedPath}[^\\n]{0,80}\\bline\\s*:?\\s*(\\d+)`, 'i'),
    /\bline\s*:?\s*(\d+)\b/i,
    /\bL(\d+)\b/i,
  ];

  for (const pattern of patterns) {
    const match = pattern.exec(block);
    if (match) {
      const line = Number(match[1]);
      if (Number.isInteger(line) && line > 0) {
        return line;
      }
    }
  }

  return null;
}

function stripParserHints(block) {
  return block
    .replace(/<!--\s*codex-review-comment\s*\n[\s\S]*?-->/g, '')
    .replace(/```codex-review-comments?\s*\n[\s\S]*?```/g, '')
    .trim();
}

function inferMarkdownPayloads(markdown, changedLineMap) {
  if (/No blocking findings found\./i.test(markdown)) {
    return [];
  }

  const comments = [];
  const paths = Array.from(changedLineMap.keys()).sort((left, right) => right.length - left.length);
  const blocks = splitFindingBlocks(markdown);

  for (const block of blocks) {
    if (/codex-review-comments?/.test(block) || /codex-review-comment/.test(block)) {
      continue;
    }

    for (const path of paths) {
      if (!block.includes(path)) {
        continue;
      }

      const line = extractLine(block, path);
      const body = stripParserHints(block);
      const comment = normalizeCommentPayload({ path, line, body }, changedLineMap);

      if (comment) {
        comments.push(comment);
        break;
      }
    }
  }

  return comments;
}

function dedupeComments(comments) {
  const seen = new Set();
  const unique = [];

  for (const comment of comments) {
    const key = `${comment.path}:${comment.line}:${comment.body}`;
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    unique.push(comment);
  }

  return unique;
}

function extractReviewComments(markdown, files) {
  const changedLineMap = buildChangedLineMap(files);
  const comments = [
    ...parseFencedPayloads(markdown, changedLineMap),
    ...parseHiddenPayloads(markdown, changedLineMap),
    ...inferMarkdownPayloads(markdown, changedLineMap),
  ];

  return dedupeComments(comments);
}

function buildReviewPayload(rawBody, files) {
  const body = normalizeReviewBody(rawBody);
  const comments = extractReviewComments(body, files);

  if (comments.length === 0) {
    return {
      body: `${CODEX_REVIEW_MARKER}\n${body}`,
    };
  }

  return {
    body: [
      CODEX_REVIEW_MARKER,
      '## Codex Review',
      '',
      `Posted ${comments.length} inline review comment${comments.length === 1 ? '' : 's'}.`,
    ].join('\n'),
    comments,
  };
}

module.exports = {
  buildChangedLineMap,
  buildReviewPayload,
  extractReviewComments,
};
