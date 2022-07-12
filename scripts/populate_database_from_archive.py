########################################################################
# Populates the OpenShaiya archive database, from a local copy of the
# archive files.
########################################################################
import sqlite3
import os
import re
import datetime
import configparser
import zlib

# The regex for finding a patches number, and date.
PATCH_REGEX = re.compile(r"ps(\d{4})-(\d{1,2})-(\d{1,2})-(\d{4})")

# The regex for finding the fullclient directory
FULLCLIENT_REGEX = re.compile(r"(ep)(\d*\.?\d*)")

# The retained files, which are not in the data folder.
RETAINED_FILES = [".ini", ".dll", ".txt", ".exe", ".cfg"]

# The valid distributions
ORIGINAL_DISTRIBUTIONS = ["us", "de", "pt", "ga", "es"]

# The valid distributions, and their paths
DISTRIBUTIONS = [
    ("us", "shaiya-us/patches", False),
    ("de", "shaiya-de/patches", False),
    ("pt", "shaiya-pt/patches", False),
    ("ga", "shaiya-gamigo/patches", False),
    ("es", "shaiya-es/patches", False),

    # These distributions are "special case", in that rather than containing a list of patches, they contain
    # one or many full clients. This is because patches weren't available for these clients at that time.
    ("cn", "shaiya-cn/archived", True),
    ("fr", "shaiya-fr/clients", True),
    ("kr", "shaiya-kr/clients", True),
    ("px", "shaiya-phoenix/clients", True),
    ("ru", "shaiya-ru/clients", True),
]

# The query for inserting to the files table.
INSERT_FILE_QUERY = """
INSERT OR IGNORE INTO files (distribution, patch, path, date, fileid) VALUES (
            ?,
            ?,
            ?,
            ?,
            ?
        );
"""

# The query for inserting to the filedata table.
INSERT_FILEDATA_QUERY = """
INSERT OR IGNORE INTO filedata (checksum, uncompressed_size, key) VALUES (
            ?,
            ?,
            ?
        );
"""

# The query for selecting from the filedata table.
SELECT_FILEDATA_QUERY = "SELECT id FROM filedata WHERE checksum = ?;"


# Collects a distribution's files.
def collect_distribution(absroot, path, fullclient):
    entries = []
    for root, dirs, files in os.walk(path):
        patch = None
        date = None

        if fullclient:
            matches = re.search(FULLCLIENT_REGEX, root)
            if matches is None:
                continue
            episode = matches.group(2)
            fullclient_path = ''.join(re.split(FULLCLIENT_REGEX, root)[0:3])

            # If the path contains a `Version.ini` file, we should parse the patch number from that.
            version_path = os.path.join(fullclient_path, "Version.ini")
            if os.path.exists(version_path):
                config = configparser.ConfigParser()
                config.read(version_path)
                patch = int(config["Version"]["CurrentVersion"])
            else:
                patch = int(episode)
                print(f"Couldn't find `Version.ini` for full client in {fullclient_path} - "
                      f"defaulting to episode via path ({patch})")

            data_path = os.path.join(fullclient_path, "data.sah")
            if os.path.exists(data_path):
                stat = os.stat(data_path)
                last_modified = stat.st_mtime
                date = datetime.datetime.fromtimestamp(last_modified)

        else:
            matches = re.search(PATCH_REGEX, root)
            if matches is None:
                continue
            patch = int(matches.group(1))
            day = int(matches.group(2))
            month = int(matches.group(3))
            year = int(matches.group(4))
            date = datetime.datetime(year, month, day)

        entries.extend(collect_base(absroot, root, patch, date))
    return entries


def collect_base(absroot, path, patch, date):
    entries = []
    for root, dirs, files in os.walk(path):
        if "data" in root:
            files = [file for file in files if not file.endswith(".patch") and file != "game.exe"]
            for file in files:
                abspath = os.path.join(root, file)
                key = os.path.relpath(abspath, absroot)
                relfile = abspath.split("data/")[1]
                entries.append(
                    {
                        "abspath": abspath,
                        "path": f"data/{relfile}".lower(),
                        "patch": patch,
                        "date": date,
                        "key": key
                    }
                )
        else:
            files = [file for file in files if file.lower().endswith(tuple(RETAINED_FILES))]
            for file in files:
                abspath = os.path.join(root, file)
                key = os.path.relpath(abspath, absroot)
                entries.append(
                    {
                        "abspath": abspath,
                        "path": file.lower(),
                        "patch": patch,
                        "date": date,
                        "key": key
                    }
                )
    return entries


def populate_database(connection, dists, entries):
    cursor = connection.cursor()

    for entry in entries:
        # For each entry, read the file data, compute a checksum, and compress the data
        infile = open(entry["abspath"], "rb")
        data = infile.read()
        crc32 = zlib.crc32(data)
        infile.close()
        uncompressed_size = len(data)
        key = entry["key"]

        # Insert the file data.
        cursor.execute(INSERT_FILEDATA_QUERY, (crc32, uncompressed_size, key))
        connection.commit()

        # Get the filedata id
        cursor.execute(SELECT_FILEDATA_QUERY, (crc32,))
        rows = cursor.fetchall()
        fileid = rows[0][0]

        # Insert the file entry for every distribution
        for dist in dists:
            print(f"fileid={fileid}, path={entry['path']}, dist={dist}, checksum={crc32}, patch={entry['patch']}, "
                  f"date={entry['date']}, key={entry['key']}")
            # Insert the file entry
            cursor.execute(INSERT_FILE_QUERY, (dist, entry["patch"], entry["path"], entry["date"], fileid))

    connection.commit()


if __name__ == "__main__":
    # Connect to the database
    connection = sqlite3.connect("../archive.sqlite")

    # Populate the data from the US base client into all `original` distributions, as patch 0.
    baseclient = collect_base(ARCHIVE, ARCHIVE+"shaiya-us/original", 0, datetime.datetime(2007, 12, 18))
    populate_database(connection, ORIGINAL_DISTRIBUTIONS, baseclient)

    # Populate the distributions. This inserts files one by one. While this is very slow, at least
    # it doesn't crash our machine from the insane amount of data being parsed.
    for dist, path, fullclient in DISTRIBUTIONS:
        entries = collect_distribution(ARCHIVE, ARCHIVE+path, fullclient)
        populate_database(connection, [dist], entries)

    # Get the last patch of Shaiya US
    cursor = connection.cursor()
    cursor.execute("SELECT max(patch) FROM files WHERE distribution = 'us';")
    rows = cursor.fetchone()
    last_us_patch = rows[0]

    # Get the US files as of the last patch.
    cursor.execute("""SELECT path, date, id FROM (
    SELECT row_number() over (partition by file.path ORDER BY patch desc) rows, file.patch, file.path, file.date, data.id FROM files file
        INNER JOIN filedata data on data.id = file.fileid
        WHERE file.distribution = ? AND file.patch <= ?
        GROUP BY file.patch, file.path, data.checksum, data.uncompressed_size, data.key
        ORDER BY file.patch DESC
) groups WHERE groups.rows <= 1;
""", ('us', last_us_patch))
    rows = cursor.fetchall()
    for row in rows:
        path = row[0]
        date = row[1]
        id = row[2]

        print(f"sheesh {path} {date} {id}")
        cursor.execute(INSERT_FILE_QUERY, ('ga', 0, path, date, id))
    connection.commit()
