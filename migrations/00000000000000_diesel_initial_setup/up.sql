-- Your SQL goes here
create table
    id_bind (
        mac TEXT not null constraint mac_key primary key,
        id TEXT not null
    );

create unique index mac_index on id_bind (mac);

create table
    player (
        id TEXT not null constraint id_key primary key,
        username TEXT not null,
        password TEXT not null,
        synced INTEGER default false not null
    );

create unique index id_index on player (id);