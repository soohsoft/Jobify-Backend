#!/usr/bin/env python3
"""Prove the SMTP credentials work, independently of the backend.

Run before/after filling SMTP_* in .env:

    python3 scripts/smtp-check.py you@example.com

It reads the same keys the backend reads, connects with the same settings, and prints the
server's own error text. That separation matters: when a signup code does not arrive, this
tells you whether the credentials are wrong (this script fails too) or the application is
(this script succeeds), instead of leaving you guessing between the two.
"""
import os
import smtplib
import ssl
import sys
from email.message import EmailMessage
from pathlib import Path

ENV = Path(__file__).resolve().parent.parent / ".env"


def load_env() -> dict:
    values = {}
    if ENV.exists():
        for line in ENV.read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            key, _, value = line.partition("=")
            values[key.strip()] = value.strip()
    # A .env value wins over the ambient environment only if it is non-empty, so an exported
    # variable can still be used to try a different password without editing the file.
    for key in ("SMTP_HOST", "SMTP_PORT", "SMTP_TLS", "SMTP_USERNAME", "SMTP_PASSWORD", "SMTP_FROM"):
        if not values.get(key) and os.environ.get(key):
            values[key] = os.environ[key]
    return values


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    recipient = sys.argv[1]
    cfg = load_env()

    host = cfg.get("SMTP_HOST", "").strip()
    port = int(cfg.get("SMTP_PORT") or 587)
    security = (cfg.get("SMTP_TLS") or "starttls").strip().lower()
    username = cfg.get("SMTP_USERNAME", "").strip()
    # Google shows the app password in groups of four; the spaces are cosmetic and pasting
    # them back verbatim is the most common reason a valid password is rejected.
    password = cfg.get("SMTP_PASSWORD", "").replace(" ", "").strip()
    sender = (cfg.get("SMTP_FROM") or username).strip()

    print(f"host      {host or '(empty — console mode)'}")
    print(f"port      {port}  security={security}")
    print(f"username  {username or '(empty)'}")
    print(f"password  {'set (' + str(len(password)) + ' chars)' if password else '(empty)'}")
    print(f"from      {sender or '(empty)'}")

    if not host:
        print("\nSMTP_HOST is empty, so the backend logs codes instead of sending them.")
        return 1
    if not username or not password:
        print("\nSMTP_USERNAME and SMTP_PASSWORD must both be set (Gmail: the full address and a 16-char app password).")
        return 1

    message = EmailMessage()
    message["From"] = sender
    message["To"] = recipient
    message["Subject"] = "jobify SMTP check"
    message.set_content("If you are reading this, the SMTP credentials work.\n")

    try:
        if security in ("tls", "ssl", "implicit"):
            with smtplib.SMTP_SSL(host, port, timeout=30, context=ssl.create_default_context()) as smtp:
                smtp.login(username, password)
                smtp.send_message(message)
        else:
            with smtplib.SMTP(host, port, timeout=30) as smtp:
                smtp.ehlo()
                smtp.starttls(context=ssl.create_default_context())
                smtp.ehlo()
                smtp.login(username, password)
                smtp.send_message(message)
    except smtplib.SMTPAuthenticationError as err:
        print(f"\nAUTHENTICATION FAILED: {err}")
        print("Gmail answers this for: a normal account password instead of an app password,")
        print("2-Step Verification not enabled, the app password was revoked, or the password")
        print("was pasted with its display spaces.")
        return 1
    except Exception as err:  # noqa: BLE001 — the server's message IS the answer here
        print(f"\nSEND FAILED ({type(err).__name__}): {err}")
        print("If this mentions unusual activity or a web-browser sign-in, Google has flagged")
        print("this server's IP: sign in to the account and confirm the activity, then retry.")
        return 1

    print(f"\nSENT — check {recipient}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
