"""Scaffold and check a developer's AI proposal, never a runnable connection."""

import argparse
import json
import re
import sys
from pathlib import Path
from urllib.parse import urlsplit

KINDS = {"model", "connection", "engine", "workflow"}
ROUTES = {"unknown", "existing", "declarative", "provider", "native-engine", "comfyui"}
AUTH = {"unknown", "none", "api-key", "oauth", "other"}
BILLING = {"unknown", "free", "metered", "subscription", "other"}
FIELDS = {
    "": {"schema_version", "title", "kind", "apps", "problem", "sources", "integration", "review"},
    "integration": {"provider", "model", "capability", "inputs", "output_media_type", "route"},
    "review": {"auth", "billing", "data_destination", "licence_source"},
}
SECRET_KEY = re.compile(r"(^|_)(api_key|token|password|secret|authorization|cookie|credential)(_|$)", re.I)
SECRET_TEXT = re.compile(r"\bAuthorization\s*:\s*Bearer\s+\S+|-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY|\bsk-[A-Za-z0-9_-]{8,}", re.I)
TOKEN = re.compile(r"^[a-z][a-z0-9-]*$")
CAPABILITY = re.compile(r"^[a-z][a-z0-9-]*\.[a-z][a-z0-9-]*$")
MEDIA_TYPE = re.compile(r"^[a-z0-9][a-z0-9.+_-]*/[a-z0-9][a-z0-9.+_-]*$")


def draft(title, kind, apps):
    """Leave all facts the agent has not established visibly unset."""
    return {
        "schema_version": 1, "title": title, "kind": kind, "apps": apps,
        "problem": "", "sources": [],
        "integration": {
            "provider": None, "model": None, "capability": None, "inputs": [],
            "output_media_type": None, "route": "unknown",
        },
        "review": {
            "auth": "unknown", "billing": "unknown", "data_destination": None,
            "licence_source": None,
        },
    }


def https_url(value):
    """Allow a public documentation URL without embedded credentials or a query token."""
    if not isinstance(value, str) or len(value) > 2048 or any(c.isspace() for c in value):
        return False
    try:
        url = urlsplit(value)
        return (url.scheme == "https" and bool(url.hostname) and url.username is None
                and url.password is None and not url.query and not url.fragment.startswith("access_token"))
    except ValueError:
        return False


def secret_paths(value, path=""):
    """Report only locations, never values, even for unexpected nested fields."""
    if isinstance(value, dict):
        for key, child in value.items():
            location = f"{path}.{key}" if path else key
            if SECRET_KEY.search(key):
                yield location
            yield from secret_paths(child, location)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from secret_paths(child, f"{path}[{index}]")
    elif isinstance(value, str) and SECRET_TEXT.search(value):
        yield path


def validate(value, stage):
    """Check proposal metadata, not API truth, runtime safety or human approval."""
    errors = [f"{path}: possible secret; remove it" for path in secret_paths(value)]
    if not isinstance(value, dict):
        return errors + ["proposal: expected a JSON object"]
    for section, allowed in FIELDS.items():
        obj = value if not section else value.get(section)
        if not isinstance(obj, dict):
            errors.append(f"{section}: expected an object")
            continue
        errors.extend(f"{section + '.' if section else ''}{key}: unknown field" for key in obj if key not in allowed)
        errors.extend(f"{section + '.' if section else ''}{key}: missing field" for key in allowed if key not in obj)
    if value.get("schema_version") != 1 or isinstance(value.get("schema_version"), bool):
        errors.append("schema_version: expected 1")
    if not isinstance(value.get("title"), str) or not value["title"].strip():
        errors.append("title: required")
    if not isinstance(value.get("kind"), str) or value["kind"] not in KINDS:
        errors.append("kind: choose model, connection, engine or workflow")
    apps = value.get("apps")
    if not isinstance(apps, list) or not apps or any(not isinstance(app, str) or not TOKEN.fullmatch(app) for app in apps) or len(apps) != len(set(map(str, apps))):
        errors.append("apps: list unique lowercase app ids")
    if not isinstance(value.get("problem"), str) or not value["problem"].strip():
        errors.append("problem: describe what the app cannot do")
    sources = value.get("sources")
    if not isinstance(sources, list) or not sources:
        errors.append("sources: provide at least one publisher or API documentation URL")
    else:
        errors.extend(f"sources[{i}]: expected a public HTTPS documentation URL without query credentials" for i, source in enumerate(sources) if not https_url(source))
    integration = value.get("integration")
    review = value.get("review")
    if isinstance(integration, dict):
        route = integration.get("route")
        if not isinstance(route, str) or route not in ROUTES:
            errors.append("integration.route: unsupported route")
        inputs = integration.get("inputs")
        if not isinstance(inputs, list) or any(not isinstance(item, str) or not TOKEN.fullmatch(item) for item in inputs) or len(inputs) != len(set(map(str, inputs))):
            errors.append("integration.inputs: list unique lowercase input names")
        if stage == "implementation":
            if not isinstance(integration.get("provider"), str) or not TOKEN.fullmatch(integration["provider"]):
                errors.append("integration.provider: identify the provider")
            if not isinstance(integration.get("capability"), str) or not CAPABILITY.fullmatch(integration["capability"]):
                errors.append("integration.capability: identify modality.task")
            if not inputs:
                errors.append("integration.inputs: declare at least one input")
            if not isinstance(integration.get("output_media_type"), str) or not MEDIA_TYPE.fullmatch(integration["output_media_type"]):
                errors.append("integration.output_media_type: identify the result format")
            if route == "unknown":
                errors.append("integration.route: choose a supported implementation route")
            if route == "declarative" and inputs != ["prompt"]:
                errors.append("integration.route: declarative is prompt-only today; use a provider for other inputs")
            if route == "declarative" and isinstance(integration.get("output_media_type"), str) and integration["output_media_type"].startswith("model/"):
                errors.append("integration.route: mesh output cannot use the declarative route until durable mesh results are implemented and tested")
    if isinstance(review, dict):
        if not isinstance(review.get("auth"), str) or review["auth"] not in AUTH:
            errors.append("review.auth: unsupported value")
        if not isinstance(review.get("billing"), str) or review["billing"] not in BILLING:
            errors.append("review.billing: unsupported value")
        if stage == "implementation":
            if review.get("auth") == "unknown":
                errors.append("review.auth: identify the authentication method")
            if review.get("billing") == "unknown":
                errors.append("review.billing: identify billing before testing")
            destination = review.get("data_destination")
            if destination != "local" and not https_url(destination):
                errors.append("review.data_destination: use local or a public HTTPS origin")
            if not https_url(review.get("licence_source")):
                errors.append("review.licence_source: link the publisher's terms")
    return errors


def issue_body(proposal):
    """Prepare review text without claiming an implementation or calling GitHub."""
    integration = proposal["integration"]
    review = proposal["review"]
    details = [
        ("Provider", integration["provider"]),
        ("Model/version", integration["model"]),
        ("Capability", integration["capability"]),
        ("Inputs", ", ".join(integration["inputs"])),
        ("Output media type", integration["output_media_type"]),
        ("Route to assess", integration["route"]),
        ("Authentication", review["auth"]),
        ("Billing", review["billing"]),
        ("Data destination", review["data_destination"]),
        ("Publisher terms", review["licence_source"]),
    ]
    facts = "".join(f"- {name}: {value}\n" for name, value in details if value)
    return (
        f"## Problem\n{proposal['problem'].strip()}\n\n"
        f"## Suggestion\n{proposal['title'].strip()} ({proposal['kind']}).\n\n"
        f"## Apps\n{', '.join(proposal['apps'])}\n\n"
        "## Integration facts to verify\n" + facts
        + "\n## Publisher documentation\n"
        + "".join(f"- {url}\n" for url in proposal["sources"])
        + "\n## Review boundary\nThis is a suggestion, not a vetted model, working connection, "
        "user binding, approval for a paid call, or permission to merge into a live build.\n"
    )


def main(argv=None):
    """Create or check a proposal without sending, installing, binding or testing it."""
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("new", help="create a suggestion without guessing API facts")
    create.add_argument("path", type=Path)
    create.add_argument("--title", required=True)
    create.add_argument("--kind", required=True, choices=sorted(KINDS))
    create.add_argument("--app", action="append", required=True)
    check = commands.add_parser("check", help="check a suggestion or implementation record")
    check.add_argument("path", type=Path)
    check.add_argument("--stage", choices=("suggestion", "implementation"), default="suggestion")
    issue = commands.add_parser("issue", help="write a checked suggestion as a GitHub issue draft")
    issue.add_argument("path", type=Path)
    issue.add_argument("--out", required=True, type=Path)
    args = parser.parse_args(argv)
    if args.command == "new":
        value = draft(args.title, args.kind, args.app)
        if any(not TOKEN.fullmatch(app) for app in args.app):
            parser.error("app ids must be lowercase tokens")
        try:
            with args.path.open("x", encoding="utf-8") as output:
                json.dump(value, output, indent=2, ensure_ascii=False)
                output.write("\n")
        except FileExistsError:
            parser.error("proposal already exists; refusing to overwrite it")
        print("Created proposal. Add a problem and publisher documentation, then run check.")
        return 0
    try:
        if args.path.stat().st_size > 65536:
            raise ValueError("proposal exceeds 64 KiB")
        value = json.loads(args.path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError) as error:
        print(f"proposal: cannot read valid JSON ({type(error).__name__})", file=sys.stderr)
        return 1
    errors = validate(value, args.stage if args.command == "check" else "suggestion")
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    if args.command == "issue":
        try:
            with args.out.open("x", encoding="utf-8") as output:
                output.write(issue_body(value))
        except FileExistsError:
            print("issue draft already exists; refusing to overwrite it", file=sys.stderr)
            return 1
        except OSError:
            print("issue draft cannot be written", file=sys.stderr)
            return 1
        print("Issue draft written. Review it before posting; nothing was sent.")
        return 0
    print(f"{args.stage} metadata passes static checks. No service, model or app was run or approved.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
