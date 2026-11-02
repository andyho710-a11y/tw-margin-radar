"""Dedicated, opt-in webhook. Never reads or reuses OKX/Telegram credentials.

Default: structured local log only. A configured webhook sends a generic event,
not raw exceptions, source responses, hostname, environment, or trading data.
"""
import datetime as dt
import hashlib
import json
import os
import pathlib
import sys
import urllib.request
import urllib.parse


def alert(stage, state, webhook=None, opener=urllib.request.urlopen):
    now = dt.datetime.now(dt.timezone(dt.timedelta(hours=8)))
    event = {"service": "tw-margin-radar", "stage": stage, "date": now.date().isoformat(), "status": "failed"}
    print(json.dumps(event), flush=True)
    if not webhook:
        return "local_only"
    url = urllib.parse.urlsplit(webhook)
    if url.scheme != 'https' or not url.hostname or url.username or url.password:
        raise ValueError('webhook_requires_https')
    # No automatic redirects: prevents a dedicated secret URL crossing to a second host.
    identity = hashlib.sha256((event['date'] + stage).encode()).hexdigest()
    marker = state / ('alert-' + identity + '.sent')
    if marker.exists():
        return 'already_sent'
    request = urllib.request.Request(webhook, data=json.dumps(event).encode(), headers={'Content-Type': 'application/json', 'Idempotency-Key': identity}, method='POST')
    with opener(request, timeout=15) as response:
        if not 200 <= response.status < 300:
            raise RuntimeError('webhook_non_success')
    marker.write_text(now.isoformat(), encoding='ascii')
    return 'sent'


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


if __name__ == '__main__':
    stage = sys.argv[1] if len(sys.argv) == 2 and sys.argv[1] in {'prices', 'update', 'health'} else 'unknown'
    try:
        opener = urllib.request.build_opener(NoRedirect()).open
        alert(stage, pathlib.Path('/var/lib/tw-margin-radar'), os.environ.get('RADAR_ALERT_WEBHOOK'), opener)
    except Exception:
        print(json.dumps({'service': 'tw-margin-radar', 'status': 'alert_delivery_failed'}), file=sys.stderr)
        sys.exit(1)
