#!/usr/bin/env python3
import asyncio
import json
import sys

POLL_SECONDS = 10
CHECKS_APPEAR_TIMEOUT_SECONDS = 120


async def gh(*args: str) -> str:
    proc = await asyncio.create_subprocess_exec(
        "gh",
        *args,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await proc.communicate()
    if proc.returncode != 0:
        raise RuntimeError(stderr.decode().strip() or "gh command failed")
    return stdout.decode()


async def pr_info() -> dict:
    return json.loads(
        await gh(
            "pr",
            "view",
            "--json",
            "number,url,headRefOid,mergeStateStatus,mergeable,reviewDecision",
        )
    )


async def api_json(endpoint: str) -> object:
    return json.loads(await gh("api", endpoint))


def has_conflict(info: dict) -> bool:
    return info.get("mergeable") == "CONFLICTING" or info.get("mergeStateStatus") == "DIRTY"


def is_agent_ack(body: str) -> bool:
    return body.strip().startswith("[codex]")


def is_bot(user: dict) -> bool:
    login = user.get("login", "")
    return user.get("type") == "Bot" or login.endswith("[bot]")


def actionable_issue_comments(comments: list[dict]) -> list[dict]:
    return [
        comment
        for comment in comments
        if not is_agent_ack(comment.get("body") or "")
        and not comment.get("body", "").strip().startswith("@codex review")
        and not is_bot(comment.get("user") or {})
    ]


def actionable_review_comments(comments: list[dict]) -> list[dict]:
    roots = {comment.get("in_reply_to_id") or comment.get("id"): comment for comment in comments}
    acked_roots = {
        comment.get("in_reply_to_id")
        for comment in comments
        if is_agent_ack(comment.get("body") or "") and comment.get("in_reply_to_id")
    }
    return [
        comment
        for root_id, comment in roots.items()
        if root_id not in acked_roots
        and not is_agent_ack(comment.get("body") or "")
        and not is_bot(comment.get("user") or {})
    ]


async def check_feedback(pr_number: int) -> None:
    issue_comments = await api_json(f"repos/{{owner}}/{{repo}}/issues/{pr_number}/comments")
    review_comments = await api_json(f"repos/{{owner}}/{{repo}}/pulls/{pr_number}/comments")
    reviews = await api_json(f"repos/{{owner}}/{{repo}}/pulls/{pr_number}/reviews")
    if actionable_issue_comments(issue_comments) or actionable_review_comments(review_comments):
        print("Review comments detected. Address before merge.")
        raise SystemExit(2)
    blocking_reviews = [
        review
        for review in reviews
        if review.get("state") == "CHANGES_REQUESTED"
        and not is_agent_ack(review.get("body") or "")
    ]
    if blocking_reviews:
        print("Blocking review state detected. Address before merge.")
        raise SystemExit(2)


async def check_runs(head_sha: str) -> list[dict]:
    payload = await api_json(f"repos/{{owner}}/{{repo}}/commits/{head_sha}/check-runs")
    return payload.get("check_runs", [])


def checks_state(runs: list[dict]) -> tuple[bool, list[str]]:
    if not runs:
        return False, ["no checks reported"]
    pending = False
    failures = []
    latest_by_name = {}
    for run in runs:
        latest_by_name[run.get("name", "unknown")] = run
    for name, run in latest_by_name.items():
        if run.get("status") != "completed":
            pending = True
            continue
        if run.get("conclusion") not in ("success", "skipped", "neutral"):
            failures.append(f"{name}: {run.get('conclusion')}")
    if pending:
        return False, []
    return True, failures


async def watch() -> None:
    info = await pr_info()
    if has_conflict(info):
        print("PR has merge conflicts.")
        raise SystemExit(5)
    pr_number = info["number"]
    head_sha = info["headRefOid"]
    empty_wait = 0
    while True:
        current = await pr_info()
        if has_conflict(current):
            print("PR has merge conflicts.")
            raise SystemExit(5)
        if current["headRefOid"] != head_sha:
            print("PR head changed.")
            raise SystemExit(4)
        await check_feedback(pr_number)
        done, failures = checks_state(await check_runs(head_sha))
        if failures and failures != ["no checks reported"]:
            print("Checks failed:")
            for failure in failures:
                print(f"- {failure}")
            raise SystemExit(3)
        if done:
            print("Checks passed and no actionable feedback detected.")
            return
        if failures == ["no checks reported"]:
            empty_wait += POLL_SECONDS
            if empty_wait >= CHECKS_APPEAR_TIMEOUT_SECONDS:
                print("No checks reported after timeout.")
                raise SystemExit(3)
        await asyncio.sleep(POLL_SECONDS)


if __name__ == "__main__":
    try:
        asyncio.run(watch())
    except RuntimeError as error:
        print(error, file=sys.stderr)
        raise SystemExit(1) from None
