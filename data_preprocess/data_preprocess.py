import sys
import polars as pl
import json
import yaml
from typing import List, Dict, Any, Tuple
from collections import defaultdict
import re

def read_and_validate_xlsx(file_path: str) -> pl.DataFrame:
    """Read and validate XLSX file structure."""
    expected_columns = [
        "organization", "team_name_en", "team_name_zh", 
        "room", "seat", "account", "password"
    ]
    
    df = pl.read_excel(file_path)
    
    if df.columns != expected_columns:
        print("Error: Column names don't match the expected structure.")
        print(f"Expected: {expected_columns}")
        print(f"Got: {df.columns}")
        sys.exit(1)
            
    print("File validation successful. Column names are correct.")
    return df

def process_organizations(df: pl.DataFrame) -> Tuple[List[str], List[Dict[str, Any]]]:
    """Process organizations: deduplicate, verify similarities, and generate JSON."""
    print(f"Found {len(df['organization'])} organizations before processing.")
    organizations = df["organization"].unique().to_list()
    original_count = len(organizations)
    
    # Find similar organizations for user verification
    similar_groups = find_similar_organizations(organizations)
    
    if similar_groups:
        print("\nPotential duplicate organizations found:")
        for i, group in enumerate(similar_groups, 1):
            print(f"Group {i}: {group}")
            response = input("Are these the same organization? (y/n): ")
            if response.lower() == 'y':
                # In real implementation, merge these organizations
                print("Note: Organization merging would be implemented here")
    
    # Get final unique organizations list
    final_organizations = sorted(organizations)
    final_count = len(final_organizations)
    
    print(f"\nAfter processing, {final_count} unique organizations remain.")
    
    # Generate organizations JSON
    orgs_json = generate_organizations_json(final_organizations)
    
    return final_organizations, orgs_json

def find_similar_organizations(organizations: List[str]) -> List[List[str]]:
    """Find potentially similar organizations for user verification."""
    org_counts = defaultdict(list)
    
    # Group by simplified names (lowercase, no spaces/special chars)
    for org in organizations:
        simplified = re.sub(r'[^\w]', '', org.lower())
        org_counts[simplified].append(org)
    
    # Return groups with potential duplicates
    return [orgs for orgs in org_counts.values() if len(orgs) > 1]

def get_country_code(organization: str) -> str:
    """Get ISO 3166-1 alpha-3 country code for organization."""
    # Default to CHN for Chinese organizations
    # You may need to implement more sophisticated country detection
    return "CHN"

def generate_organizations_json(unique_orgs: List[str]) -> List[Dict[str, Any]]:
    """Generate organizations data in required JSON format."""
    max_width = len(str(len(unique_orgs)))
    
    result = []
    for idx, org in enumerate(unique_orgs, 1):
        org_id = f"INST-{idx:0{max_width}d}"
        result.append({
            "id": org_id,
            "icpc_id": org_id,
            "name": org,
            "formal_name": org,
            "country": get_country_code(org)
        })
    
    return result

def generate_teams_json(df: pl.DataFrame, org_mapping: Dict[str, str]) -> List[Dict[str, Any]]:
    """Generate teams data in required JSON format."""
    teams = []
    
    for row in df.iter_rows(named=True):
        org_id = org_mapping.get(row['organization'])
        if not org_id:
            continue
            
        location_desc = f"{row['room']}-{row['seat']}".strip('-')
        location = {"description": location_desc} if location_desc else {}
        
        team_data = {
            "id": row['account'],
            "icpc_id": row['account'],
            "label": row['account'],
            "display_name": row['team_name_zh'],
            "group_ids": ["participants"],
            "name": row['team_name_zh'],
            "organization_id": org_id
        }
        
        if location:
            team_data["location"] = location
            
        teams.append(team_data)
    
    return teams

def generate_accounts_yaml(df: pl.DataFrame) -> List[Dict[str, Any]]:
    """Generate accounts data in YAML format."""
    accounts = []
    
    for row in df.iter_rows(named=True):
        account_data = {
            "id": row['account'],
            "username": row['account'],
            "password": row['password'],
            "type": "team",
            "team_id": row['account'],
            "name": row['account']
        }
        accounts.append(account_data)
    
    return accounts

def main():
    if len(sys.argv) < 2:
        print("Error: Please provide the XLSX file path as an argument.")
        sys.exit(1)
    
    file_path = sys.argv[1]
    
    try:
        # Step 1: Read and validate file
        df = read_and_validate_xlsx(file_path)
        
        # Step 2: Process organizations
        final_orgs, orgs_json = process_organizations(df)
        
        # Create organization mapping (org name -> org id)
        org_mapping = {}
        for org_data in orgs_json:
            org_mapping[org_data['name']] = org_data['id']
        
        # Step 3: Generate teams JSON
        teams_json = generate_teams_json(df, org_mapping)
        
        # Step 4: Generate accounts YAML
        accounts_yaml = generate_accounts_yaml(df)
        
        # Save organizations to file
        with open("organizations.json", "w", encoding="utf-8") as f:
            json.dump(orgs_json, f, indent=2, ensure_ascii=False)
        
        # Save teams to file
        with open("teams.json", "w", encoding="utf-8") as f:
            json.dump(teams_json, f, indent=2, ensure_ascii=False)
        
        # Save accounts to file
        with open("accounts.yaml", "w", encoding="utf-8") as f:
            yaml.dump(accounts_yaml, f, allow_unicode=True, sort_keys=False)
        
        print("organizations.json, teams.json and accounts.yaml generated successfully.")
        
    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()