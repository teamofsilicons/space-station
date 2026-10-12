#!/usr/bin/env python3
"""Check deployment cleanup without touching host services or sending network traffic."""
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

HERE = Path(__file__).resolve().parent


class NoTelemetry(unittest.TestCase):
    def test_existing_forwarders_are_removed_and_backups_remain(self):
        for role in ('api', 'clickhouse'):
            for installed in (False, True):
                with self.subTest(role=role, installed=installed), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agent = root / 'opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl'
                    agent.parent.mkdir(parents=True)
                    if installed:
                        agent.touch()
                    (root / 'var/log').mkdir(parents=True)
                    with patch('pathlib.Path', side_effect=lambda path: root / str(path).lstrip('/')), \
                         patch('subprocess.run') as run, patch.object(sys, 'argv', ['operations.py', role]):
                        runpy.run_path(str(HERE / 'operations.py'), run_name='__main__')
                    commands = [call.args[0] for call in run.call_args_list]
                    removal = [('systemctl', 'disable', '--now', 'amazon-cloudwatch-agent'),
                               ('dpkg', '--purge', 'amazon-cloudwatch-agent')]
                    self.assertEqual([cmd for cmd in commands if 'amazon-cloudwatch-agent' in cmd],
                                     removal if installed else [])
                    self.assertIn(('systemctl', 'enable', '--now', 'space-station-backup.timer'), commands)
                    backup = root / 'opt/space-station/bin/backup'
                    self.assertIn('aws s3 cp', backup.read_text())
                    self.assertNotIn('cloudwatch', backup.read_text())
                    self.assertIn('pg_dump' if role == 'api' else 'BACKUP DATABASE', backup.read_text())
                    subprocess.run(['bash', '-n', str(backup)], check=True)

    def test_infrastructure_does_not_enable_reporting(self):
        template = json.loads(subprocess.check_output([sys.executable, str(HERE / 'template.py')]))
        self.assertEqual(template, json.loads((HERE / 'stack.json').read_text()))
        for resource in template['Resources'].values():
            self.assertNotIn(resource['Type'], ('AWS::CloudWatch::Alarm', 'AWS::Logs::LogGroup'))
            props = resource['Properties']
            if resource['Type'] == 'AWS::EC2::Instance':
                self.assertFalse(props['Monitoring'])
            if resource['Type'] == 'AWS::IAM::Role':
                self.assertTrue(all('CloudWatch' not in policy for policy in props.get('ManagedPolicyArns', [])))
        firewall = template['Resources']['GatewayFirewall']['Properties']
        for visibility in [firewall['VisibilityConfig'], *(rule['VisibilityConfig'] for rule in firewall['Rules'])]:
            self.assertFalse(visibility['CloudWatchMetricsEnabled'])
            self.assertFalse(visibility['SampledRequestsEnabled'])
        reporting = ET.parse(HERE / 'clickhouse-no-telemetry.xml').getroot().find('send_crash_reports')
        self.assertEqual(reporting.findtext('enabled'), 'false')
        self.assertEqual(reporting.findtext('send_logical_errors'), 'false')


if __name__ == '__main__':
    unittest.main()
