"""Resolve the claim catalog that applies to a measured source version."""
import json


def load_catalog(root, release=None):
    directory = root / 'site/app/releases'
    catalog = json.loads((directory / 'claims.json').read_text())
    assessments_path = directory / 'assessments.json'
    assessments = json.loads(assessments_path.read_text()) if assessments_path.exists() else {}
    overrides = assessments.get(release, {}).get('catalog', {})
    excluded_claims = set(overrides.get('excludeClaims', []))
    excluded_challenges = set(overrides.get('excludeChallenges', []))
    for key in ('claims', 'challenges'):
        catalog[key] = [dict(item, **overrides.get(key, {}).get(item['id'], {}))
                        for item in catalog[key]
                        if item['id'] not in excluded_claims | excluded_challenges
                        and item.get('claim') not in excluded_claims]
    retained_kits = {item.get('kit') for item in catalog['challenges']}
    catalog['kits'] = {key: dict(value, **overrides.get('kits', {}).get(key, {}))
                       for key, value in catalog['kits'].items() if key in retained_kits}
    return catalog
