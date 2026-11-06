import { ApiError } from "../api/errors";
import type {
  ExpectedPhoto,
  PhotoDecision_Serialize as PhotoDecision,
  PhotoPreviewEntry_Serialize as PhotoPreviewEntry,
  PhotoRecord,
  PhotoSource,
  ResultImportFailure,
  ResultImportState,
  SubmissionState,
} from "../api/generated/types";
import type { Tone } from "../components/device-status-style";

export function photoManagementError(error: unknown) {
  const fallback =
    "Photo request failed. Refresh and verify the result before repeating an operation.";
  if (!(error instanceof ApiError)) return fallback;
  if (error.status === 409)
    return "Photo state changed or the roster, source, or import no longer allows this operation. Refresh and verify before trying again.";
  if (error.status === 403) return "Administrator access is required.";
  if (error.status === 404)
    return "This photo record no longer exists. Refresh the list.";
  if (error.status === 413) return "The package or export exceeds its limit.";
  if (error.status === 503)
    return "The server is busy with another photo operation. Try again shortly.";
  if (error.status === 400)
    return "The request was rejected. Check the values or package and try again.";
  return fallback;
}

export interface SourceStatus {
  label: string;
  tone: Tone;
}

/** Operator-facing lifecycle of one Photo source, in precedence order. */
export function sourceStatus(source: PhotoSource): SourceStatus {
  if (source.state === "revoked") return { label: "Revoked", tone: "muted" };
  if (source.state === "disabled")
    return { label: "Disabled", tone: "warning" };
  if (source.initialization_completed)
    return { label: "Initialized", tone: "success" };
  if (source.enrollment_confirmed)
    return { label: "Awaiting initialization", tone: "info" };
  return { label: "Awaiting pairing", tone: "muted" };
}

export interface SourceActions {
  issueCode: boolean;
  resetEnrollment: boolean;
  enable: boolean;
  disable: boolean;
  revoke: boolean;
}

/**
 * Mirrors the Server's rules: codes and enrollment replacement need an
 * enabled source that has not completed initialization; a confirmed binding
 * can only be replaced explicitly; revocation is permanent.
 */
export function sourceActions(source: PhotoSource): SourceActions {
  const enabled = source.state === "enabled";
  const open = enabled && !source.initialization_completed;
  return {
    issueCode: open && !source.enrollment_confirmed,
    resetEnrollment: open,
    enable: source.state === "disabled",
    disable: enabled,
    revoke: source.state !== "revoked",
  };
}

export const submissionStates: Record<
  SubmissionState,
  { label: string; tone: Tone }
> = {
  active: { label: "Current", tone: "success" },
  pending_review: { label: "Needs review", tone: "warning" },
  needs_association: { label: "Needs team", tone: "warning" },
  rejected: { label: "Rejected", tone: "muted" },
  superseded: { label: "Superseded", tone: "muted" },
};

export const importStates: Record<
  ResultImportState,
  { label: string; tone: Tone }
> = {
  uploading: { label: "Uploading", tone: "info" },
  processing: { label: "Verifying package", tone: "info" },
  ready: { label: "Ready to commit", tone: "warning" },
  committed: { label: "Committed", tone: "success" },
  failed: { label: "Failed", tone: "danger" },
};

export const importFailures: Record<ResultImportFailure, string> = {
  limit: "The package exceeds a size, entry, or image limit.",
  input: "The package is not a valid result ZIP.",
  storage: "The Server could not store the package.",
  archive: "The archive structure or contents are invalid.",
  signature:
    "The manifest signature or its initialization context was rejected.",
  authorization: "The signing source is unknown, disabled, or revoked.",
  image: "An image is missing, corrupt, or doesn’t match its manifest.",
  conflict: "The package conflicts with results already accepted.",
  interrupted: "Processing was interrupted by a Server restart. Upload again.",
};

/** Import jobs that still change without operator action. */
export function importInFlight(state: ResultImportState) {
  return state === "uploading" || state === "processing";
}

export function shortHash(hash: string) {
  return hash.slice(0, 12);
}

export function reviewable(entry: Pick<PhotoPreviewEntry, "state">) {
  return (
    entry.state === "pending_review" || entry.state === "needs_association"
  );
}

/** Candidate that must be mapped by hand because its team left the roster. */
export function needsTarget(entry: Pick<PhotoPreviewEntry, "state">) {
  return entry.state === "needs_association";
}

function expectedPhoto(
  record: { source_id: string; submission_id: string } | null | undefined,
): ExpectedPhoto | null {
  return record
    ? { source_id: record.source_id, submission_id: record.submission_id }
    : null;
}

/**
 * Optimistic-concurrency token for the photo currently shown for `target`.
 * The preview already carries it for the candidate's own team; other targets
 * are looked up in the current photo list. The Server rejects stale tokens.
 */
export function expectedCurrent(
  entry: PhotoPreviewEntry,
  target: string,
  photos: readonly PhotoRecord[],
): ExpectedPhoto | null {
  if (target === entry.account_id) return expectedPhoto(entry.current);
  return expectedPhoto(photos.find((photo) => photo.account_id === target));
}

export function rejectDecision(entry: PhotoPreviewEntry): PhotoDecision {
  return {
    submission_id: entry.submission_id,
    expected_submission_revision: entry.submission_revision,
    action: { action: "reject" },
  };
}

export function activateDecision(
  entry: PhotoPreviewEntry,
  target: string,
  photos: readonly PhotoRecord[],
): PhotoDecision {
  return {
    submission_id: entry.submission_id,
    expected_submission_revision: entry.submission_revision,
    action: {
      action: "activate",
      account_id: target,
      expected_current: expectedCurrent(entry, target, photos),
    },
  };
}

export interface PreviewCounts {
  reviewable: number;
  current: number;
  other: number;
}

export function previewCounts(entries: readonly PhotoPreviewEntry[]) {
  return entries.reduce<PreviewCounts>(
    (total, entry) => {
      if (reviewable(entry)) total.reviewable += 1;
      else if (entry.state === "active") total.current += 1;
      else total.other += 1;
      return total;
    },
    { reviewable: 0, current: 0, other: 0 },
  );
}
