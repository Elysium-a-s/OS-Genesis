import copy
import json
import unittest
from pathlib import Path

from jsonschema import Draft202012Validator, FormatChecker


ROOT = Path(__file__).resolve().parents[1]
SCHEMA_PATH = ROOT / "v1" / "message.schema.json"
EXAMPLES = ROOT / "v1" / "examples"


class GenesisV1ContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
        Draft202012Validator.check_schema(cls.schema)
        cls.validator = Draft202012Validator(cls.schema, format_checker=FormatChecker())

    def sample(self, name):
        return json.loads((EXAMPLES / name).read_text(encoding="utf-8"))

    def assert_invalid(self, value):
        self.assertFalse(self.validator.is_valid(value))

    def test_all_examples_validate(self):
        files = sorted(EXAMPLES.glob("*.json"))
        self.assertEqual(len(files), 7)
        for path in files:
            with self.subTest(example=path.name):
                self.validator.validate(json.loads(path.read_text(encoding="utf-8")))

    def test_command_requires_identity_and_idempotency(self):
        command = self.sample("command-unknown.json")
        for key in ("actor", "idempotency_key", "correlation_id", "household_id"):
            with self.subTest(missing=key):
                invalid = copy.deepcopy(command)
                del invalid[key]
                self.assert_invalid(invalid)

    def test_confirmation_cannot_be_claimed_without_matching_evidence(self):
        provider = self.sample("command-provider-confirmed.json")
        device = self.sample("command-device-confirmed.json")
        missing_evidence = copy.deepcopy(provider)
        del missing_evidence["evidence"]
        self.assert_invalid(missing_evidence)
        wrong_evidence = copy.deepcopy(device)
        wrong_evidence["evidence"]["kind"] = "provider_ack"
        self.assert_invalid(wrong_evidence)
        unknown_device_confirmed = self.sample("command-unknown.json")
        unknown_device_confirmed["confirmation_level"] = "device"
        self.assert_invalid(unknown_device_confirmed)

    def test_unknown_observation_has_no_claimed_value(self):
        observation = self.sample("observation.json")
        observation["quality"] = "unknown"
        self.assert_invalid(observation)
        observation["value"] = None
        self.validator.validate(observation)

    def test_version_status_and_timestamp_are_constrained(self):
        command = self.sample("command-unknown.json")
        for field, value in (
            ("schema_version", "2.0"),
            ("status", "success"),
            ("requested_at", "2026-09-29T10:05:00+02:00"),
        ):
            with self.subTest(field=field):
                invalid = copy.deepcopy(command)
                invalid[field] = value
                self.assert_invalid(invalid)

    def test_unrecognized_fields_are_rejected(self):
        device = self.sample("device.json")
        device["access_token"] = "must-not-be-in-contract"
        self.assert_invalid(device)


if __name__ == "__main__":
    unittest.main()
