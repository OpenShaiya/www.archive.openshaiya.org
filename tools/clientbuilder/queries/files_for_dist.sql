SELECT path, key, uncompressed_size, date FROM (
    SELECT row_number() over (partition by file.path ORDER BY patch desc) rows, file.patch, file.path, file.date, data.checksum, data.uncompressed_size, data.key FROM files file
        INNER JOIN filedata data on data.id = file.fileid
        WHERE file.distribution = ? AND file.patch <= ?
        GROUP BY file.patch, file.path, data.checksum, data.uncompressed_size, data.key
        ORDER BY file.patch DESC
) groups WHERE groups.rows <= 1;