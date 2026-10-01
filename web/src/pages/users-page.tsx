import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "react-router";

import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { DataState } from "@/components/data-state";
import { Alert, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { operatorLinkURL } from "@/operator/links";
import { useOperatorAction } from "@/operator/use-operator-action";

type Account = components["schemas"]["OperatorAccountResponse"];
type Invitation = components["schemas"]["OperatorInvitationResponse"];
type Role = Account["role"];
type IssuedLink = {
  url: string;
  description: string;
  expires: number;
  reset: boolean;
};
type Confirmation = { account: Account; role?: Role };
const selectClass =
  "h-9 rounded-md border bg-background px-3 text-sm disabled:opacity-50";

function dateLabel(value: number) {
  return new Date(value).toLocaleString();
}

export function UsersPage() {
  const scope = useSessionScope();
  const navigate = useNavigate();
  const action = useOperatorAction();
  const [role, setRole] = useState<Role>("viewer");
  const [issued, setIssued] = useState<IssuedLink | null>(null);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const accounts = useQuery({
    queryKey: ["operator-accounts"],
    queryFn: async ({ signal }) =>
      unwrap<Account[]>(
        await scope.api.GET("/api/v2/operator/accounts", { signal }),
      ),
    refetchInterval: LIST_POLL_MS,
  });
  const invitations = useQuery({
    queryKey: ["operator-invitations"],
    queryFn: async ({ signal }) =>
      unwrap<Invitation[]>(
        await scope.api.GET("/api/v2/operator/invitations", { signal }),
      ),
    refetchInterval: LIST_POLL_MS,
  });
  const onlyAdmin =
    accounts.data?.filter((account) => account.role === "admin").length === 1;

  function refresh() {
    void scope.queryClient.invalidateQueries({
      queryKey: ["operator-accounts"],
    });
    void scope.queryClient.invalidateQueries({
      queryKey: ["operator-invitations"],
    });
  }

  function issueInvitation(invitation?: Invitation) {
    setIssued(null);
    void action.run(
      async (signal) =>
        unwrap(
          invitation
            ? await scope.api.POST(
                "/api/v2/operator/invitations/{invite_id}/actions/regenerate",
                {
                  params: { path: { invite_id: invitation.invitation_id } },
                  signal,
                },
              )
            : await scope.api.POST("/api/v2/operator/invitations", {
                body: { role },
                signal,
              }),
        ),
      (data) => {
        setIssued({
          reset: false,
          url: operatorLinkURL(window.location.origin, "register", data.token),
          description: `Registration link · ${data.invitation.role.toUpperCase()}`,
          expires: data.invitation.expires_at_unix_ms,
        });
        refresh();
      },
    );
  }

  function issueReset(account: Account) {
    setIssued(null);
    void action.run(
      async (signal) =>
        unwrap(
          await scope.api.POST(
            "/api/v2/operator/accounts/{operator_id}/password-resets",
            { params: { path: { operator_id: account.operator_id } }, signal },
          ),
        ),
      (data) => {
        setIssued({
          reset: true,
          url: operatorLinkURL(
            window.location.origin,
            "reset-password",
            data.token,
          ),
          description: `Password reset link for ${data.username}`,
          expires: data.expires_at_unix_ms,
        });
      },
    );
  }

  function confirmAccountChange(change: Confirmation) {
    setIssued(null);
    void action.run(
      async (signal) => {
        const params = { path: { operator_id: change.account.operator_id } };
        await unwrap<void>(
          change.role
            ? await scope.api.PATCH("/api/v2/operator/accounts/{operator_id}", {
                params,
                body: { role: change.role },
                signal,
              })
            : await scope.api.DELETE(
                "/api/v2/operator/accounts/{operator_id}",
                { params, signal },
              ),
        );
      },
      () => {
        setConfirmation(null);
        if (change.account.operator_id === scope.identity?.operator_id) {
          if (change.role) {
            scope.observe({ ...scope.identity, role: change.role });
            navigate("/seats", { replace: true });
          } else {
            scope.logout();
            navigate("/login", {
              replace: true,
              state: { operatorNotice: "Your user account was deleted." },
            });
          }
        } else refresh();
      },
    );
  }

  return (
    <div className="min-w-0 space-y-8">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold">Users</h1>
          <p className="text-sm text-muted-foreground">
            Manage Natsume users and registration invitations.
          </p>
        </div>
        <Button
          variant="outline"
          disabled={action.pending}
          onClick={() => {
            setIssued(null);
            refresh();
          }}
        >
          Refresh
        </Button>
      </div>
      {action.error && (
        <Alert variant="destructive">
          <AlertTitle className="line-clamp-none">{action.error}</AlertTitle>
        </Alert>
      )}
      {issued && (
        <IssuedOperatorLink
          key={issued.url}
          link={issued}
          onDismiss={() => setIssued(null)}
        />
      )}
      <section aria-label="User accounts" className="space-y-3">
        <h2 className="text-lg font-semibold">User accounts</h2>
        <p className="text-sm text-muted-foreground">
          All administrators have equal access. The last administrator must
          remain. Demoting an administrator revokes their unused registration
          and reset links.
        </p>
        <DataState
          isLoading={accounts.isLoading}
          error={accounts.error}
          isEmpty={!accounts.data?.length}
          emptyLabel="No users found."
        >
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Username</TableHead>
                <TableHead>Role</TableHead>
                <TableHead>User ID</TableHead>
                <TableHead>Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {accounts.data?.map((account) => {
                const self =
                  account.operator_id === scope.identity?.operator_id;
                const lastAdmin = onlyAdmin && account.role === "admin";
                return (
                  <TableRow key={account.operator_id}>
                    <TableCell className="min-w-40">
                      <div className="flex max-w-xs flex-wrap items-center gap-2">
                        <span className="whitespace-normal break-all">
                          {account.username}
                        </span>
                        {self && <Badge variant="secondary">You</Badge>}
                        {lastAdmin && (
                          <Badge variant="outline">Last admin</Badge>
                        )}
                      </div>
                    </TableCell>
                    <TableCell>
                      <select
                        aria-label={`Role for ${account.username}`}
                        value={account.role}
                        disabled={action.pending || lastAdmin}
                        className={selectClass}
                        onChange={(event) =>
                          setConfirmation({
                            account,
                            role: event.target.value as Role,
                          })
                        }
                      >
                        <option value="viewer">Viewer</option>
                        <option value="admin">Admin</option>
                      </select>
                    </TableCell>
                    <TableCell className="font-mono text-xs">
                      {account.operator_id}
                    </TableCell>
                    <TableCell>
                      <div className="flex gap-2">
                        <Button
                          variant="outline"
                          size="sm"
                          disabled={action.pending}
                          onClick={() => issueReset(account)}
                        >
                          Reset password
                        </Button>
                        <Button
                          variant="destructive"
                          size="sm"
                          disabled={action.pending || lastAdmin}
                          onClick={() => setConfirmation({ account })}
                        >
                          Delete
                        </Button>
                      </div>
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </DataState>
        {accounts.error && (
          <Button variant="outline" onClick={() => accounts.refetch()}>
            Retry users
          </Button>
        )}
      </section>
      <section aria-label="Registration invitations" className="space-y-3">
        <h2 className="text-lg font-semibold">Registration invitations</h2>
        <p className="text-sm text-muted-foreground">
          Invitations last 7 days and register one user. The recipient chooses a
          username and password. Regenerating revokes the old link and keeps its
          role.
        </p>
        <form
          className="flex flex-wrap items-end gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            issueInvitation();
          }}
        >
          <div className="space-y-2">
            <Label htmlFor="invitation-role">Invitation role</Label>
            <select
              id="invitation-role"
              className={`${selectClass} block`}
              value={role}
              disabled={action.pending}
              onChange={(event) => setRole(event.target.value as Role)}
            >
              <option value="viewer">Viewer</option>
              <option value="admin">Admin</option>
            </select>
          </div>
          <Button type="submit" disabled={action.pending}>
            Create invitation
          </Button>
        </form>
        <DataState
          isLoading={invitations.isLoading}
          error={invitations.error}
          isEmpty={!invitations.data?.length}
          emptyLabel="No pending invitations."
        >
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Role</TableHead>
                <TableHead>Issued by</TableHead>
                <TableHead>Created</TableHead>
                <TableHead>Expires</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {invitations.data?.map((invitation) => (
                <TableRow key={invitation.invitation_id}>
                  <TableCell>{invitation.role.toUpperCase()}</TableCell>
                  <TableCell>
                    {accounts.data?.find(
                      (account) =>
                        account.operator_id === invitation.issuer_operator_id,
                    )?.username ?? invitation.issuer_operator_id}
                  </TableCell>
                  <TableCell>
                    {dateLabel(invitation.created_at_unix_ms)}
                  </TableCell>
                  <TableCell>
                    {dateLabel(invitation.expires_at_unix_ms)}
                  </TableCell>
                  <TableCell>
                    <Badge
                      variant={invitation.expired ? "outline" : "secondary"}
                    >
                      {invitation.expired ? "Expired" : "Pending"}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    <div className="flex gap-2">
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={action.pending}
                        onClick={() => issueInvitation(invitation)}
                      >
                        Regenerate
                      </Button>
                      <Button
                        variant="destructive"
                        size="sm"
                        disabled={action.pending}
                        onClick={() => {
                          setIssued(null);
                          void action.run(async (signal) => {
                            await unwrap<void>(
                              await scope.api.DELETE(
                                "/api/v2/operator/invitations/{invite_id}",
                                {
                                  params: {
                                    path: {
                                      invite_id: invitation.invitation_id,
                                    },
                                  },
                                  signal,
                                },
                              ),
                            );
                          }, refresh);
                        }}
                      >
                        Revoke
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </DataState>
        {invitations.error && (
          <Button variant="outline" onClick={() => invitations.refetch()}>
            Retry invitations
          </Button>
        )}
      </section>
      <AlertDialog
        open={confirmation !== null}
        onOpenChange={(open) => {
          if (!open && !action.pending) setConfirmation(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {confirmation?.role ? "Change user role?" : "Delete user?"}
            </AlertDialogTitle>
            <AlertDialogDescription className="break-words">
              {confirmation?.role
                ? `Change ${confirmation.account.username} to ${confirmation.role.toUpperCase()}? Demoting an administrator revokes their unused links.`
                : `Delete ${confirmation?.account.username}? Their sessions and unused links will be revoked.`}
              {confirmation?.account.operator_id ===
                scope.identity?.operator_id && " This is your own account."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {action.error && (
            <Alert variant="destructive">
              <AlertTitle className="line-clamp-none">
                {action.error}
              </AlertTitle>
            </Alert>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={action.pending}>
              Cancel
            </AlertDialogCancel>
            <Button
              variant={confirmation?.role ? "default" : "destructive"}
              disabled={action.pending}
              onClick={() => {
                if (confirmation) confirmAccountChange(confirmation);
              }}
            >
              {action.pending
                ? "Saving..."
                : confirmation?.role
                  ? "Confirm role change"
                  : "Delete user"}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function IssuedOperatorLink({
  link,
  onDismiss,
}: {
  link: IssuedLink;
  onDismiss: () => void;
}) {
  const copy = useOperatorAction();
  const [copied, setCopied] = useState(false);
  return (
    <section aria-label="New link" className="space-y-3 rounded-md border p-4">
      <h2 className="break-all font-semibold">{link.description}</h2>
      <p className="text-sm text-muted-foreground">
        Expires {dateLabel(link.expires)}. Copy now; this link is only shown
        once.{" "}
        {link.reset
          ? "Valid for 1 hour. A new reset link replaces the previous one without changing the user's current password."
          : "Valid for 7 days and one registration."}
      </p>
      <Label htmlFor="issued-link">Link</Label>
      <Input
        id="issued-link"
        readOnly
        value={link.url}
        onFocus={(event) => event.target.select()}
      />
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={copy.pending}
          onClick={() =>
            copy.run(
              () => navigator.clipboard.writeText(link.url),
              () => setCopied(true),
            )
          }
        >
          {copied ? "Copied" : "Copy link"}
        </Button>
        <Button variant="outline" onClick={onDismiss}>
          Dismiss
        </Button>
      </div>
      {copy.error && (
        <p role="alert" className="text-sm text-destructive">
          Unable to copy. Select the link and copy it manually.
        </p>
      )}
    </section>
  );
}
