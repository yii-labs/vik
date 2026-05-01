pub const CANDIDATE_QUERY: &str = r#"
query VikCandidateIssues($projectSlug: String!, $activeStates: [String!], $first: Int!, $after: String) {
  issues(
    first: $first
    after: $after
    filter: {
      project: { slugId: { eq: $projectSlug } }
      state: { name: { in: $activeStates } }
    }
  ) {
    nodes {
      id
      identifier
      title
      description
      priority
      branchName
      url
      createdAt
      updatedAt
      state { name }
      labels { nodes { name } }
      inverseRelations {
        nodes {
          type
          issue { id identifier state { name } }
        }
      }
    }
    pageInfo { hasNextPage endCursor }
  }
}
"#;

pub const ISSUES_BY_STATES_QUERY: &str = r#"
query VikIssuesByStates($projectSlug: String!, $stateNames: [String!], $first: Int!, $after: String) {
  issues(
    first: $first
    after: $after
    filter: {
      project: { slugId: { eq: $projectSlug } }
      state: { name: { in: $stateNames } }
    }
  ) {
    nodes {
      id
      identifier
      title
      description
      priority
      branchName
      url
      createdAt
      updatedAt
      state { name }
      labels { nodes { name } }
      inverseRelations {
        nodes {
          type
          issue { id identifier state { name } }
        }
      }
    }
    pageInfo { hasNextPage endCursor }
  }
}
"#;

pub const ISSUE_STATES_BY_IDS_QUERY: &str = r#"
query VikIssueStatesByIds($ids: [ID!]!) {
  issues(filter: { id: { in: $ids } }) {
    nodes {
      id
      identifier
      title
      state { name }
      updatedAt
    }
  }
}
"#;

pub const ISSUE_BY_IDENTIFIER_QUERY: &str = r#"
query VikIssueByIdentifier($id: String!) {
  issue(id: $id) {
    id
    identifier
    attachments(first: 50) {
      nodes {
        id
        title
        url
      }
    }
  }
}
"#;

pub const ATTACHMENT_CREATE_MUTATION: &str = r#"
mutation VikAttachmentCreate($input: AttachmentCreateInput!) {
  attachmentCreate(input: $input) {
    success
    attachment {
      id
      title
      url
      issue {
        id
        identifier
      }
    }
  }
}
"#;
