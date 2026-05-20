Fetch current GitHub Project item and issue detail:

!`exec(gh issue view {{ issue.id }} --json number,title,body,state,labels,comments,url,updatedAt)`

Project item id: `{{ issue.project_item_id }}`
Project status: `{{ issue.state }}`
