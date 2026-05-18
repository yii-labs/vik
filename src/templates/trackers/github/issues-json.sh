gh issue list --label "vik" --state "open" --limit 50 \
  --search 'label:__STAGE_LABELS__ -label:blocked sort:created-asc' \
  --json number,title,labels \
  --jq '
    [
      .[]
      | ([.labels[].name] | map(select(__JQ_STATES__))) as $states
      | select($states | length == 1)
      | { id: (.number | tostring), title: .title, state: $states[0] }
    ]
  '
