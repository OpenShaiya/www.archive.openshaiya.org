-- A function for getting all relevant files of a distribution, as of a given patch number.
-- This only returns the most recent files, up to and including the provided patch number.
CREATE FUNCTION get_files_for_distribution
(
    in_distribution      shaiya_distribution,
    in_patch             smallint
)
RETURNS TABLE (file_patch smallint, file_path text, file_checksum bigint, file_uncompressed_size bigint, file_data bytea)
AS $$
BEGIN
    RETURN QUERY (
        SELECT patch, path, checksum, uncompressed_size, data FROM (
            SELECT row_number() over (partition by file.path ORDER BY patch desc) rows, file.patch, file.path, data.checksum, data.uncompressed_size, data.data FROM public.files file
            INNER JOIN public.filedata data on data.id = file.fileid
            WHERE file.distribution = in_distribution AND file.patch <= in_patch
            GROUP BY file.patch, file.path, data.checksum, data.uncompressed_size, data.data
            ORDER BY file.patch DESC
        ) groups WHERE groups.rows <= 1
    );
END
$$ LANGUAGE plpgsql;
