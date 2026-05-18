: "${LINEAR_API_KEY:?LINEAR_API_KEY is required}"
TEAM_KEY="${LINEAR_TEAM_KEY:-ENG}"

QUERY='
query ($teamKey: String!) {
  issues(
    filter: { team: { key: { eq: $teamKey } } }
    first: 50
    orderBy: createdAt
  ) {
    nodes {
      identifier
      title
      state { name }
    }
  }
}'

curl -sS https://api.linear.app/graphql \
  -H "Authorization: $LINEAR_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$(jq -n --arg q "$QUERY" --arg teamKey "$TEAM_KEY" '{query: $q, variables: {teamKey: $teamKey}}')" \
| jq '
    [
      .data.issues.nodes[]
      | { id: .identifier, title: .title, state: .state.name }
    ]
  '
