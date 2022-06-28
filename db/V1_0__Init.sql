CREATE TYPE shaiya_distribution AS ENUM('us', 'de', 'pt', 'ga', 'es', 'cn', 'fr', 'kr', 'px', 'ru');

CREATE TABLE filedata (
    id              bigint GENERATED ALWAYS AS IDENTITY,
    path            text NOT NULL,
    checksum        bigint NOT NULL,
    data            bytea,
    PRIMARY KEY     (id),
    UNIQUE          (path, checksum)
);

CREATE TABLE files (
    id              bigint GENERATED ALWAYS AS IDENTITY,
    distribution    shaiya_distribution NOT NULL,
    patch           smallint NOT NULL,
    date            date NOT NULL,
    fileid          bigint NOT NULL REFERENCES filedata(id),
    UNIQUE          (distribution, fileid)
);