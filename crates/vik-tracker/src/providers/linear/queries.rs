pub const CANDIDATE_QUERY: &str = r#"
query VikCandidateIssues(
  $projectSlug: String!
  $activeStates: [String!]
  $assigneeFilter: NullableUserFilter! = {}
  $labelFilter: IssueLabelCollectionFilter! = {}
  $first: Int!
  $after: String
) {
  issues(
    first: $first
    after: $after
    filter: {
      project: { slugId: { eq: $projectSlug } }
      state: { name: { in: $activeStates } }
      assignee: $assigneeFilter
      labels: $labelFilter
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

pub const ISSUE_BY_ID_QUERY: &str = r#"
query VikIssueById($id: String!) {
  issue(id: $id) {
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

pub const ISSUE_STATES_FOR_ISSUE_QUERY: &str = r#"
query VikIssueStatesForIssue($id: String!) {
  issue(id: $id) {
    id
    labels {
      nodes {
        id
        name
      }
    }
    team {
      states {
        nodes {
          id
          name
        }
      }
      labels {
        nodes {
          id
          name
        }
      }
    }
  }
}
"#;

pub const ISSUE_UPDATE_MUTATION: &str = r#"
mutation VikIssueUpdate($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) {
    success
    issue {
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
  }
}
"#;

pub const COMMENT_CREATE_MUTATION: &str = r#"
mutation VikCommentCreate($issueId: String!, $body: String!) {
  commentCreate(input: { issueId: $issueId, body: $body }) {
    success
    comment {
      id
      body
      url
    }
  }
}
"#;

pub const ISSUE_COMMENTS_QUERY: &str = r#"
query VikIssueComments($id: String!, $first: Int!, $after: String) {
  issue(id: $id) {
    comments(first: $first, after: $after) {
      nodes {
        id
        body
        url
      }
      pageInfo { hasNextPage endCursor }
    }
  }
}
"#;

pub const COMMENT_UPDATE_MUTATION: &str = r#"
mutation VikCommentUpdate($id: String!, $body: String!) {
  commentUpdate(id: $id, input: { body: $body }) {
    success
    comment {
      id
      body
      url
    }
  }
}
"#;

pub const FILE_UPLOAD_MUTATION: &str = r#"
mutation VikFileUpload(
  $filename: String!
  $contentType: String!
  $size: Int!
  $makePublic: Boolean
) {
  fileUpload(
    filename: $filename
    contentType: $contentType
    size: $size
    makePublic: $makePublic
  ) {
    success
    uploadFile {
      uploadUrl
      assetUrl
      headers {
        key
        value
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
