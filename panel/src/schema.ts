import * as z from "zod";


export const InfoSchema = z.object({
    "mac": z.union([z.null(), z.string()]),
    "id": z.string(),
    "ip": z.union([z.null(), z.string()]),
    "last_seen": z.union([z.null(), z.string()]),
    "username": z.union([z.null(), z.string()]),
    "password": z.union([z.null(), z.string()]),
    "synced": z.union([z.boolean(), z.null()]),
});
export type Info = z.infer<typeof InfoSchema>;

export const StatusResponseSchema = z.object({
    "bind_count": z.number(),
    "info_count": z.number(),
    "sync_count": z.number(),
    "notsync_count": z.number(),
    "infos": z.array(InfoSchema),
});
export type StatusResponse = z.infer<typeof StatusResponseSchema>;

export const ErrorResponseSchema = z.object({
    "error": z.string(),
    "msg": z.string(),
});
export type ErrorResponse = z.infer<typeof ErrorResponseSchema>;
