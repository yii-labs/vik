: "${LINEAR_API_KEY:?LINEAR_API_KEY is required}"
TEAM_KEY="${LINEAR_TEAM_KEY:-ENG}"
STATES='__STATE_NAMES_JSON__'

QUERY='
query ($teamKey: String!, $states: [String!]!) {
  issues(
    filter: {
      team: { key: { eq: $teamKey } }
      state: { name: { in: $states } }
    }
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

response=$(curl -sS https://api.linear.app/graphql \
  -H "Authorization: $LINEAR_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$(jq -n --arg q "$QUERY" --arg teamKey "$TEAM_KEY" --argjson states "$STATES" '{query: $q, variables: {teamKey: $teamKey, states: $states}}')") || exit $?

printf '%s\n' "$response" \
| jq '
    [
      .data.issues.nodes[]
      | { id: .identifier, title: .title, state: .state.name }
    ]
  '
