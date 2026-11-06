import { describe, expect, it } from "vitest";

import { ApiError } from "../api/errors";
import type {
  PhotoPreviewEntry_Serialize as PhotoPreviewEntry,
  PhotoRecord,
  PhotoSource,
} from "../api/generated/types";
import {
  activateDecision,
  importInFlight,
  needsTarget,
  photoManagementError,
  previewCounts,
  rejectDecision,
  reviewable,
  sourceActions,
  sourceStatus,
} from "./photo-management";

const source: PhotoSource = {
  source_id: "source",
  name: "Station",
  state: "enabled",
  enrollment_confirmed: false,
  initialization_completed: false,
};

const entry: PhotoPreviewEntry = {
  submission_id: "submission",
  result_id: "result",
  account_id: "account-a",
  account: "team001",
  image_sha256: "0".repeat(64),
  state: "pending_review",
  submission_revision: 3,
  current: {
    source_id: "s",
    submission_id: "old",
    revision: 2,
    image_sha256: "1".repeat(64),
  },
  active_account_id: null,
};

const photo: PhotoRecord = {
  account_id: "account-b",
  account: "team002",
  source_id: "s2",
  submission_id: "other",
  revision: 4,
  image_sha256: "2".repeat(64),
  image_format: "jpeg",
  image_bytes: 10,
};

describe("photo source lifecycle", () => {
  it("maps state precedence to a status", () => {
    expect(sourceStatus(source).label).toBe("Awaiting pairing");
    expect(sourceStatus({ ...source, enrollment_confirmed: true }).label).toBe(
      "Awaiting initialization",
    );
    expect(
      sourceStatus({
        ...source,
        enrollment_confirmed: true,
        initialization_completed: true,
      }).label,
    ).toBe("Initialized");
    expect(sourceStatus({ ...source, state: "disabled" }).tone).toBe("warning");
    expect(
      sourceStatus({
        ...source,
        state: "revoked",
        initialization_completed: true,
      }).label,
    ).toBe("Revoked");
  });

  it("offers only operations the Server accepts", () => {
    expect(sourceActions(source)).toEqual({
      issueCode: true,
      resetEnrollment: true,
      enable: false,
      disable: true,
      revoke: true,
    });
    expect(
      sourceActions({ ...source, enrollment_confirmed: true }).issueCode,
    ).toBe(false);
    expect(
      sourceActions({ ...source, initialization_completed: true }),
    ).toMatchObject({
      issueCode: false,
      resetEnrollment: false,
      disable: true,
    });
    expect(sourceActions({ ...source, state: "disabled" })).toEqual({
      issueCode: false,
      resetEnrollment: false,
      enable: true,
      disable: false,
      revoke: true,
    });
    expect(sourceActions({ ...source, state: "revoked" })).toEqual({
      issueCode: false,
      resetEnrollment: false,
      enable: false,
      disable: false,
      revoke: false,
    });
  });
});

describe("photo decisions", () => {
  it("rejects with the revision seen in the preview", () => {
    expect(rejectDecision(entry)).toEqual({
      submission_id: "submission",
      expected_submission_revision: 3,
      action: { action: "reject" },
    });
  });

  it("activates the own team against the previewed current photo", () => {
    expect(activateDecision(entry, "account-a", [photo]).action).toEqual({
      action: "activate",
      account_id: "account-a",
      expected_current: { source_id: "s", submission_id: "old" },
    });
  });

  it("looks up the current photo of a reassigned target", () => {
    const orphan = { ...entry, state: "needs_association" as const };
    expect(activateDecision(orphan, "account-b", [photo]).action).toEqual({
      action: "activate",
      account_id: "account-b",
      expected_current: { source_id: "s2", submission_id: "other" },
    });
    expect(activateDecision(orphan, "account-c", [photo]).action).toMatchObject(
      { expected_current: null },
    );
  });

  it("classifies reviewable entries and counts them", () => {
    const orphan = { ...entry, state: "needs_association" as const };
    expect(reviewable(entry)).toBe(true);
    expect(needsTarget(entry)).toBe(false);
    expect(needsTarget(orphan)).toBe(true);
    expect(
      previewCounts([
        entry,
        orphan,
        { ...entry, state: "active" },
        { ...entry, state: "superseded" },
        { ...entry, state: "rejected" },
      ]),
    ).toEqual({ reviewable: 2, current: 1, other: 2 });
  });
});

describe("photo helpers", () => {
  it("polls only imports the Server is still working on", () => {
    expect(importInFlight("uploading")).toBe(true);
    expect(importInFlight("processing")).toBe(true);
    for (const state of ["ready", "committed", "failed"] as const)
      expect(importInFlight(state)).toBe(false);
  });

  it("explains conflicts without exposing server detail", () => {
    expect(photoManagementError(new ApiError("X", 409, "Conflict"))).toMatch(
      /state changed/,
    );
    expect(photoManagementError(new Error("boom"))).toMatch(/Photo request/);
  });
});
