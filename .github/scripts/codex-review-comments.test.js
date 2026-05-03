'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const { buildReviewPayload, extractReviewComments } = require('./codex-review-comments.js');

const files = [
  {
    filename: 'crates/vik-agent/src/process.rs',
    patch: [
      '@@ -218,6 +218,7 @@',
      ' context 218',
      ' context 219',
      '+new line 220',
      ' context 221',
    ].join('\n'),
  },
];

test('extracts JSON-first review comments', () => {
  const comments = extractReviewComments(
    JSON.stringify({
      comments: [
        {
          path: 'crates/vik-agent/src/process.rs',
          line: 220,
          body: 'Route delayed turn messages before logging.',
        },
      ],
    }),
    files,
  );

  assert.deepEqual(comments, [
    {
      path: 'crates/vik-agent/src/process.rs',
      line: 220,
      side: 'RIGHT',
      body: 'Route delayed turn messages before logging.',
    },
  ]);
});

test('extracts fenced JSON review comments with ranges', () => {
  const comments = extractReviewComments(
    [
      '## Codex Review',
      '',
      '```codex-review-comments',
      JSON.stringify({
        comments: [
          {
            path: '/tmp/github-runner-workdir/vik/vik/review/crates/vik-agent/src/process.rs',
            start_line: 219,
            line: 220,
            body: 'Range body.',
          },
        ],
      }),
      '```',
    ].join('\n'),
    files,
  );

  assert.deepEqual(comments, [
    {
      path: 'crates/vik-agent/src/process.rs',
      line: 220,
      side: 'RIGHT',
      body: 'Range body.',
      start_line: 219,
      start_side: 'RIGHT',
    },
  ]);
});

test('keeps hidden block compatibility', () => {
  const comments = extractReviewComments(
    [
      '## Codex Review',
      '',
      '<!-- codex-review-comment',
      'path: crates/vik-agent/src/process.rs',
      'line: 220',
      'body:',
      'Hidden body.',
      '-->',
    ].join('\n'),
    files,
  );

  assert.deepEqual(comments, [
    {
      path: 'crates/vik-agent/src/process.rs',
      line: 220,
      side: 'RIGHT',
      body: 'Hidden body.',
    },
  ]);
});

test('parses duplicated plain-text Codex review findings cleanly', () => {
  const dash = '\u2014';
  const sample = [
    'The new session logging can record delayed messages from another turn into the wrong session file, so the added feature is not correct for multi-turn runs with out-of-order/delayed events.',
    '',
    'Review comment:',
    '',
    `- [P2] Route delayed turn messages before logging ${dash} /tmp/github-runner-workdir/vik/vik/review/crates/vik-agent/src/process.rs:220-220`,
    '  In multi-turn runs, if the app-server emits an event for the previous turn after the next `turn/start` response, this unconditional append writes that stale message into the new turn session file before `wait_for_turn` checks the turn id.',
    'The new session logging can record delayed messages from another turn into the wrong session file, so the added feature is not correct for multi-turn runs with out-of-order/delayed events.',
    '',
    'Review comment:',
    '',
    `- [P2] Route delayed turn messages before logging ${dash} /tmp/github-runner-workdir/vik/vik/review/crates/vik-agent/src/process.rs:220-220`,
    '  In multi-turn runs, if the app-server emits an event for the previous turn after the next `turn/start` response, this unconditional append writes that stale message into the new turn session file before `wait_for_turn` checks the turn id.',
  ].join('\n');

  const payload = buildReviewPayload(sample, files);

  assert.equal(payload.comments.length, 1);
  assert.deepEqual(payload.comments[0], {
    path: 'crates/vik-agent/src/process.rs',
    line: 220,
    side: 'RIGHT',
    body: [
      '[P2] Route delayed turn messages before logging',
      '',
      'In multi-turn runs, if the app-server emits an event for the previous turn after the next `turn/start` response, this unconditional append writes that stale message into the new turn session file before `wait_for_turn` checks the turn id.',
    ].join('\n'),
  });
  assert.equal(payload.body, '<!-- codex-review -->\n## Codex Review\n\nPosted 1 inline review comment.');
});

test('keeps no-finding output as one non-inline review body', () => {
  const payload = buildReviewPayload('## Codex Review\n\nNo blocking findings found.', files);

  assert.deepEqual(payload, {
    body: '<!-- codex-review -->\n## Codex Review\n\nNo blocking findings found.',
  });
});
