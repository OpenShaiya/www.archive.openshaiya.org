CREATE TABLE filedata (
    id                  integer PRIMARY KEY AUTOINCREMENT ,
    checksum            bigint NOT NULL,
    uncompressed_size   bigint NOT NULL,
    key                 text,
    UNIQUE              (checksum)
);

CREATE TABLE files (
    id              integer PRIMARY KEY AUTOINCREMENT,
    distribution    text NOT NULL,
    patch           smallint NOT NULL,
    path            text NOT NULL,
    date            date NOT NULL,
    fileid          bigint NOT NULL REFERENCES filedata(id),
    UNIQUE          (distribution, path, fileid)
);