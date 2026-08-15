export interface PreparationPreview {
  candidate_id: string;
  preview_token: string;
}

let preview: PreparationPreview | null = null;

export function getPreparationPreview(): PreparationPreview | null {
  return preview;
}

export function setPreparationPreview(value: PreparationPreview): void {
  preview = { ...value };
}

export function clearPreparationPreview(): void {
  preview = null;
}
