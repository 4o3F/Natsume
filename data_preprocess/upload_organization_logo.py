import sys
import os
import polars as pl
import requests
from typing import List, Set, Dict
import re
import base64

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

def get_unique_organizations(df: pl.DataFrame) -> List[str]:
    """Get unique organizations from dataframe."""
    organizations = df["organization"].unique().to_list()
    print(f"Found {len(organizations)} unique organizations.")
    return organizations

def normalize_filename(filename: str) -> str:
    """Normalize filename by removing extension and special characters."""
    # Remove file extension
    name = os.path.splitext(filename)[0]
    # Remove special characters and convert to lowercase
    normalized = re.sub(r'[^\w]', '', name.lower())
    return normalized

def match_organizations_with_files(organizations: List[str], folder_path: str) -> Dict[str, str]:
    """Match organizations with files in folder and return organization to filename mapping."""
    if not os.path.exists(folder_path):
        print(f"Error: Folder path '{folder_path}' does not exist.")
        sys.exit(1)
    
    # Get all filenames in folder
    filenames = os.listdir(folder_path)
    print(f"Found {len(filenames)} files in folder.")
    
    # Create mapping from normalized filename to actual filename
    file_mapping = {}
    for fname in filenames:
        normalized = normalize_filename(fname)
        file_mapping[normalized] = fname
    
    # Create organization to filename mapping
    org_file_mapping = {}
    for org in organizations:
        normalized_org = normalize_filename(org)
        if normalized_org in file_mapping:
            org_file_mapping[org] = file_mapping[normalized_org]
    
    return org_file_mapping

def get_organization_id_mapping(organizations: List[str]) -> Dict[str, str]:
    """Generate organization ID mapping (org name -> INST-id)."""
    org_id_mapping = {}
    for idx, org in enumerate(sorted(organizations), 1):
        org_id = f"INST-{idx:03d}"  # Format as INST-001, INST-002, etc.
        org_id_mapping[org] = org_id
    return org_id_mapping

def upload_logo(api_domain: str, org_id: str, file_path: str, auth_token: str) -> bool:
    """Upload logo file to API."""
    url = f"{api_domain}/api/v4/organizations/{org_id}/logo"
    
    try:
        with open(file_path, 'rb') as f:
            files = {'logo': f}
            headers = {
                'Authorization': f'Basic {auth_token}',
            }
            
            response = requests.post(url, files=files, headers=headers)
            
            if response.status_code == 204:
                print(f"✓ Successfully uploaded logo for {org_id}")
                return True
            else:
                print(f"✗ Failed to upload logo for {org_id}: {response.status_code} - {response.text}")
                return False
                
    except Exception as e:
        print(f"✗ Error uploading logo for {org_id}: {e}")
        return False

def main():
    if len(sys.argv) < 4:
        print("Error: Please provide XLSX file path, folder path, and API domain as arguments.")
        print("Usage: python script.py <xlsx_file> <folder_path> <api_domain>")
        sys.exit(1)
    
    xlsx_file = sys.argv[1]
    folder_path = sys.argv[2]
    api_domain = sys.argv[3]
    
    # Get Basic Auth credentials from user
    username = input("Enter API username: ")
    password = input("Enter API password: ")
    auth_token = base64.b64encode(f"{username}:{password}".encode()).decode()
    
    try:
        # Step 1: Read and validate XLSX file
        df = read_and_validate_xlsx(xlsx_file)
        
        # Step 2: Get unique organizations
        organizations = get_unique_organizations(df)
        
        # Step 3: Match organizations with files
        org_file_mapping = match_organizations_with_files(organizations, folder_path)
        
        # Step 4: Generate organization ID mapping
        org_id_mapping = get_organization_id_mapping(organizations)
        
        # Step 5: Upload logos via API
        print("\nStarting logo uploads...")
        success_count = 0
        total_count = len(org_file_mapping)
        
        for org_name, filename in org_file_mapping.items():
            org_id = org_id_mapping[org_name]
            file_path = os.path.join(folder_path, filename)
            
            if upload_logo(api_domain, org_id, file_path, auth_token):
                success_count += 1
        
        # Output results
        print(f"\nUpload completed: {success_count}/{total_count} successful")
        
        # Show unmatched organizations
        unmatched = set(organizations) - set(org_file_mapping.keys())
        if unmatched:
            print(f"\n{len(unmatched)} organizations without matching files:")
            for org in sorted(unmatched):
                print(f"  - {org}")
            
    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()