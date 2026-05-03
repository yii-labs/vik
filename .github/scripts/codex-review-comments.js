'use strict';

const CODEX_REVIEW_MARKER = '<!-- codex-review -->';
const sortedChangedPathsCache = new WeakMap();

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

function sortedChangedPaths(changedLineMap) {
  const cached = sortedChangedPathsCache.get(changedLineMap);
  if (cached) {
    return cached;
  }

  const paths = Array.from(changedLineMap.keys()).sort((left, right) => right.length - left.length);
  sortedChangedPathsCache.set(changedLineMap, paths);
  return paths;
}

function findChangedPath(path, changedLineMap) {
  const normalizedPath = String(path || '').trim();

  if (!normalizedPath) {
    return '';
  }

  if (changedLineMap.has(normalizedPath)) {
    return normalizedPath;
  }

  return sortedChangedPaths(changedLineMap).find((candidate) => normalizedPath.endsWith(`/${candidate}`) || normalizedPath === candidate) || '';
}

function parsePositiveInteger(value) {
  const number = Number(value);
  return Number.isInteger(number) && number > 0 ? number : null;
}

function hasVisibleLineRange(changedLines, startLine, endLine) {
  for (let line = startLine; line <= endLine; line += 1) {
    if (!changedLines.has(line)) {
      return false;
    }
  }

  return true;
}

function normalizeCommentPayload(payload, changedLineMap) {
  if (!payload || typeof payload !== 'object') {
    return null;
  }

  const path = findChangedPath(payload.path, changedLineMap);
  const line = parsePositiveInteger(payload.line);
  const startLine = parsePositiveInteger(payload.start_line);
  const body = String(payload.body || '').trim();

  if (!path || !line || !body) {
    return null;
  }

  const changedLines = changedLineMap.get(path);
  if (!changedLines || !changedLines.has(line)) {
    return null;
  }

  const comment = {
    path,
    line,
    side: 'RIGHT',
    body,
  };

  if (startLine && startLine < line && hasVisibleLineRange(changedLines, startLine, line)) {
    comment.start_line = startLine;
    comment.start_side = 'RIGHT';
  }

  return comment;
}

function collectPayloads(value) {
  if (Array.isArray(value)) {
    return value;
  }

  if (value && Array.isArray(value.comments)) {
    return value.comments;
  }

  if (value && Array.isArray(value.findings)) {
    return value.findings.map(findingToPayload);
  }

  if (value && typeof value === 'object') {
    return [value];
  }

  return [];
}

function parsePriorityLabel(value) {
  const number = Number(value);
  if (Number.isInteger(number) && number >= 0 && number <= 3) {
    return `P${number}`;
  }

  const match = /^P([0-3])$/i.exec(String(value || '').trim());
  return match ? `P${match[1]}` : '';
}

function hasPriorityPrefix(title) {
  return /^\[?P[0-3]\]?(?:\s|:)/i.test(title);
}

function findingToPayload(finding) {
  if (!finding || typeof finding !== 'object') {
    return null;
  }

  const codeLocation = finding.code_location || {};
  const lineRange = codeLocation.line_range || finding.line_range || {};
  const startLine = lineRange.start || lineRange.start_line;
  const endLine = lineRange.end || lineRange.end_line || startLine;
  const priority = parsePriorityLabel(finding.priority ?? finding.severity);
  const title = String(finding.title || finding.summary || 'Codex finding').trim();
  const detail = String(finding.body || finding.description || finding.explanation || '').trim();
  const heading = priority && !hasPriorityPrefix(title) ? `[${priority}] ${title}` : title;

  return {
    path: codeLocation.absolute_file_path || codeLocation.path || finding.path || finding.file || finding.filename || '',
    start_line: startLine,
    line: endLine,
    body: [heading, detail].filter(Boolean).join('\n\n'),
  };
}

function collectNormalizedPayloads(value, changedLineMap) {
  const comments = [];

  for (const payload of collectPayloads(value)) {
    const comment = normalizeCommentPayload(payload, changedLineMap);
    if (comment) {
      comments.push(comment);
    }
  }

  return comments;
}

function parseJsonPayloads(markdown, changedLineMap) {
  try {
    return collectNormalizedPayloads(JSON.parse(markdown), changedLineMap);
  } catch (_) {
    return [];
  }
}

function parseFencedPayloads(markdown, changedLineMap) {
  const comments = [];
  const fencePattern = /```codex-review-comments?\s*\n([\s\S]*?)```/g;
  let match;

  while ((match = fencePattern.exec(markdown)) !== null) {
    try {
      comments.push(...collectNormalizedPayloads(JSON.parse(match[1]), changedLineMap));
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
    const startLine = /^start_line:\s*(\d+)$/im.exec(block);
    const body = /^body:\s*\n([\s\S]*)$/im.exec(block);

    const comment = normalizeCommentPayload(
      {
        path: path ? path[1].trim() : '',
        line: line ? line[1] : '',
        start_line: startLine ? startLine[1] : '',
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
      /^\s*(?:[-*]|\d+[.)])\s+(?:\*\*)?(?:\[?P[0-3]\]?|critical|high|medium|low|blocking|finding|issue|bug)(?:\b|\s|:)/i.test(line);

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

function parseReviewCommentBlocks(markdown, changedLineMap) {
  const comments = [];
  const consumedHeadings = new Set();
  const lines = markdown.split(/\r?\n/);
  const paths = sortedChangedPaths(changedLineMap);

  for (let index = 0; index < lines.length; index += 1) {
    const heading = /^\s*[-*]\s+\[(P[0-3])\]\s+(.+?)\s+(?:\u2014|-)\s+(\S+):(\d+)(?:-(\d+))?\s*$/.exec(lines[index]);
    if (!heading) {
      continue;
    }

    const [, severity, title, rawPath, startRaw, endRaw] = heading;
    const path = findChangedPath(rawPath, changedLineMap);
    if (!path || !paths.includes(path)) {
      continue;
    }

    const line = parsePositiveInteger(endRaw || startRaw);
    const startLine = parsePositiveInteger(startRaw);
    const detailLines = [];

    for (let detailIndex = index + 1; detailIndex < lines.length; detailIndex += 1) {
      const detail = lines[detailIndex];
      const nextHeading = /^\s*[-*]\s+\[(P[0-3])\]\s+/.test(detail);

      if (nextHeading) {
        break;
      }

      if (detail.trim() === 'Review comment:') {
        break;
      }

      if (detail.trim() && !/^\s+/.test(detail)) {
        break;
      }

      detailLines.push(detail.replace(/^\s{1,2}/, ''));
    }

    const detail = detailLines.join('\n').trim();
    const body = [`[${severity}] ${title.trim()}`, detail].filter(Boolean).join('\n\n');
    const comment = normalizeCommentPayload(
      {
        path,
        line,
        start_line: startLine,
        body,
      },
      changedLineMap,
    );

    if (comment) {
      comments.push(comment);
      consumedHeadings.add(lines[index].trim());
    }
  }

  return { comments, consumedHeadings };
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

function inferMarkdownPayloads(markdown, changedLineMap, consumedHeadings = new Set()) {
  if (/No blocking findings found\./i.test(markdown)) {
    return [];
  }

  const comments = [];
  const paths = sortedChangedPaths(changedLineMap);
  const blocks = splitFindingBlocks(markdown);

  for (const block of blocks) {
    if (/codex-review-comments?/.test(block) || /codex-review-comment/.test(block)) {
      continue;
    }

    const firstLine = block.split(/\r?\n/, 1)[0].trim();
    if (consumedHeadings.has(firstLine)) {
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

function commentDedupeKey(comment) {
  return [
    comment.path,
    comment.start_line || '',
    comment.start_side || '',
    comment.line,
    comment.side || '',
    comment.body,
  ].join('\0');
}

function dedupeComments(comments) {
  const seen = new Set();
  const unique = [];

  for (const comment of comments) {
    const key = commentDedupeKey(comment);
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
  const reviewCommentBlocks = parseReviewCommentBlocks(markdown, changedLineMap);
  const structuredComments = [
    ...parseJsonPayloads(markdown, changedLineMap),
    ...parseFencedPayloads(markdown, changedLineMap),
    ...parseHiddenPayloads(markdown, changedLineMap),
    ...reviewCommentBlocks.comments,
  ];
  const comments = [
    ...structuredComments,
    ...inferMarkdownPayloads(markdown, changedLineMap, reviewCommentBlocks.consumedHeadings),
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
