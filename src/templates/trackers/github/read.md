Fetch current GitHub issue detail:

!`exec(gh issue view {{ issue.id }} --json number,title,body,state,labels,comments,url,updatedAt)`
