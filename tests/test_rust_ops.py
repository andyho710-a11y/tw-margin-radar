"""Offline checks; webhook calls are fakes and no notification is sent."""
import importlib.util
import pathlib
import tempfile
import unittest
from unittest.mock import Mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('radar_alert', ROOT / 'ops/alert.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
isolation_spec = importlib.util.spec_from_file_location('radar_isolation', ROOT / 'ops/verify_isolation.py')
isolation = importlib.util.module_from_spec(isolation_spec)
isolation_spec.loader.exec_module(isolation)


class OpsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        (ROOT / 'var/test-tmp').mkdir(parents=True, exist_ok=True)

    def test_alert_disabled_by_default(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'var/test-tmp') as d:
            opener = Mock()
            self.assertEqual(module.alert('health', pathlib.Path(d), opener=opener), 'local_only')
            opener.assert_not_called()

    def test_alert_dedup_and_failure_retry(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'var/test-tmp') as d:
            state = pathlib.Path(d)
            opener = Mock(side_effect=TimeoutError())
            with self.assertRaises(TimeoutError):
                module.alert('health', state, 'https://example.com/dedicated-hook', opener)
            self.assertEqual(list(state.glob('*.sent')), [])
            response = Mock()
            response.status = 204
            response.__enter__ = Mock(return_value=response)
            response.__exit__ = Mock(return_value=False)
            opener = Mock(return_value=response)
            self.assertEqual(module.alert('health', state, 'https://example.com/dedicated-hook', opener), 'sent')
            self.assertEqual(module.alert('health', state, 'https://example.com/dedicated-hook', opener), 'already_sent')
            self.assertEqual(opener.call_count, 1)
            self.assertNotIn(b'dedicated-hook', opener.call_args.args[0].data)

    def test_insecure_webhook_is_rejected(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'var/test-tmp') as d:
            for url in ['http://example.com/hook', 'https://user:pass@example.com/hook']:
                with self.assertRaises(ValueError):
                    module.alert('health', pathlib.Path(d), url, Mock())

    def test_timers_use_taipei_and_expected_retry_windows(self):
        prices = (ROOT / 'ops/systemd/tw-margin-radar-prices.timer').read_text()
        update = (ROOT / 'ops/systemd/tw-margin-radar-update.timer').read_text()
        health = (ROOT / 'ops/systemd/tw-margin-radar-health.timer').read_text()
        self.assertIn('16:10:00 Asia/Taipei', prices)
        for time in ['18:40', '20:10', '22:10', '23:10']:
            self.assertIn(time + ':00 Asia/Taipei', update)
        self.assertIn('23:40:00 Asia/Taipei', health)
        self.assertIn('Persistent=true', update)

    def test_service_separates_bot_access_and_limits_resources(self):
        service = (ROOT / 'ops/systemd/tw-margin-radar@.service').read_text()
        for setting in ['User=tw-radar', 'MemoryMax=192M', 'CPUQuota=20%', 'TasksMax=24', 'ProtectHome=true', 'ProtectSystem=strict', 'NoNewPrivileges=true', 'IPAddressDeny=169.254.0.0/16']:
            self.assertIn(setting, service)
        self.assertNotIn('pm2', service)
        self.assertNotIn('/home/', service)

    def test_isolation_probe_uses_reviewed_service_policy_and_private_dns(self):
        service = (ROOT / 'ops/systemd/tw-margin-radar@.service').read_text()
        properties = isolation.policy(service)
        self.assertIn(('BindReadOnlyPaths', '/etc/tw-margin-radar/resolv.conf:/etc/resolv.conf'), properties)
        self.assertIn(('IPAddressDeny', '169.254.0.0/16'), properties)
        self.assertIn(('IPAddressDeny', 'fd20:ce::254/128'), properties)
        resolver = (ROOT / 'ops/network/resolv.conf').read_text()
        self.assertIn('nameserver 8.8.8.8', resolver)
        self.assertIn('nameserver 1.1.1.1', resolver)
        self.assertNotIn('nameserver 169.254.', resolver)
        with self.assertRaises(ValueError):
            isolation.policy(service.replace('ProtectHome=true', ''))

    def test_review_ci_has_no_deployment_permissions(self):
        workflow = (ROOT / '.github/workflows/rust-validate.yml').read_text()
        self.assertIn('contents: read', workflow)
        self.assertNotIn('schedule:', workflow)
        self.assertNotIn('pages: write', workflow)
        self.assertNotIn('secrets.', workflow)


if __name__ == '__main__':
    unittest.main()
