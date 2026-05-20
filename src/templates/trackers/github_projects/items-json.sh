: "${GITHUB_PROJECT_OWNER:?GITHUB_PROJECT_OWNER is required}"
: "${GITHUB_PROJECT_NUMBER:?GITHUB_PROJECT_NUMBER is required}"
GITHUB_PROJECT_QUERY="${GITHUB_PROJECT_QUERY:-is:issue}"

gh project item-list "$GITHUB_PROJECT_NUMBER" \
  --owner "$GITHUB_PROJECT_OWNER" \
  --limit "${GITHUB_PROJECT_LIMIT:-50}" \
  --format json \
  --query "$GITHUB_PROJECT_QUERY" \
  --jq '
    [
      .items[]
      | select((.content.type // .type) == "Issue")
      | select(.status | __JQ_STATES__)
      | {
          id: (.content.number | tostring),
          title: .content.title,
          state: .status,
          project_status: .status,
          project_item_id: .id,
          project_owner: env.GITHUB_PROJECT_OWNER,
          project_number: env.GITHUB_PROJECT_NUMBER,
          url: .content.url,
          repository: (.content.repository // .repository)
        }
    ]
  '
