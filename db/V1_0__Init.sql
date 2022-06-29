CREATE TYPE shaiya_distribution AS ENUM('us', 'de', 'pt', 'ga', 'es', 'cn', 'fr', 'kr', 'px', 'ru');

CREATE TABLE filedata (
    id                  bigint GENERATED ALWAYS AS IDENTITY,
    checksum            bigint NOT NULL,
    uncompressed_size   bigint NOT NULL,
    data                bytea,
    PRIMARY KEY         (id),
    UNIQUE              (checksum)
);

CREATE TABLE files (
    id              bigint GENERATED ALWAYS AS IDENTITY,
    distribution    shaiya_distribution NOT NULL,
    patch           smallint NOT NULL,
    path            text NOT NULL,
    date            date NOT NULL,
    fileid          bigint NOT NULL REFERENCES filedata(id),
    UNIQUE          (distribution, path, fileid)
);