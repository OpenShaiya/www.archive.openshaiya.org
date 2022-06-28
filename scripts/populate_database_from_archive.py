########################################################################
# Populates the OpenShaiya archive database, from a local copy of the
# archive files.
########################################################################
import psycopg2
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
RETAINED_FILES = [".ini", ".dll", ".txt", ".exe"]

# The valid distributions, and their paths
DISTRIBUTIONS = [
    ("us", "archive/shaiya-us/patches", False),
    ("de", "archive/shaiya-de/patches", False),
    ("pt", "archive/shaiya-pt/patches", False),
    ("ga", "archive/shaiya-gamigo/patches", False),
    ("es", "archive/shaiya-es/patches", False),

    # These distributions are "special case", in that rather than containing a list of patches, they contain
    # one or many full clients. This is because patches weren't available for these clients at that time.
    ("cn", "archive/shaiya-cn/archived", True),
    ("fr", "archive/shaiya-fr/clients", True),
    ("kr", "archive/shaiya-kr/clients", True),
    ("px", "archive/shaiya-phoenix/clients", True),
    ("ru", "archive/shaiya-ru/clients", True)
]

# The query for inserting to the files table.
INSERT_FILE_QUERY = """
INSERT INTO public.files (distribution, patch, date, fileid) VALUES (
            %s,
            %s,
            %s,
            %s
        ) ON CONFLICT DO NOTHING;
"""

# The query for inserting to the filedata table.
INSERT_FILEDATA_QUERY = """
INSERT INTO public.filedata (path, checksum, data) VALUES (
            %s,
            %s,
            %s
        ) ON CONFLICT DO NOTHING;
"""

# The query for selecting from the filedata table.
SELECT_FILEDATA_QUERY = "SELECT id FROM public.filedata WHERE path = %s AND checksum = %s;"

# Collects a distribution's files.
def collect_distribution(path, fullclient):
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

        if "data" in root:
            files = [file for file in files if not file.endswith(".patch") and file != "game.exe"]
            for file in files:
                abspath = os.path.join(root, file)
                relfile = abspath.split("data/")[1]

                entries.append(
                    {
                        "abspath": abspath,
                        "path": f"data/{relfile}",
                        "patch": patch,
                        "date": date,
                    }
                )
        else:
            files = [file for file in files if file.endswith(tuple(RETAINED_FILES))]
            for file in files:
                abspath = os.path.join(root, file)
                entries.append(
                    {
                        "abspath": abspath,
                        "path": file,
                        "patch": patch,
                        "date": date,
                    }
                )
    return entries


if __name__ == "__main__":
    # Connect to the pgsql database
    connection = psycopg2.connect(user="postgres", password="postgres", host="localhost", port="5432",
                                  database="shaiyaarchive")

    # Populate the distributions. This inserts files one by one. While this is very slow, at least
    # it doesn't crash our machine from the insane amount of data being parsed.
    for dist, path, fullclient in DISTRIBUTIONS:
        for entry in collect_distribution(path, fullclient):
            # For each entry, read the file data, compute a checksum, and compress the data
            infile = open(entry["abspath"], "rb")
            data = infile.read()
            crc32 = zlib.crc32(data)
            infile.close()
            compressed_data = zlib.compress(data)
            cursor = connection.cursor()

            # Insert the file data.
            cursor.execute(INSERT_FILEDATA_QUERY, (entry["path"], crc32, compressed_data))
            connection.commit()

            # Get the filedata id
            cursor.execute(SELECT_FILEDATA_QUERY, (entry["path"], crc32))
            rows = cursor.fetchall()
            fileid = rows[0][0]
            print(f"fileid={fileid}, path={entry['path']}, dist={dist}, checksum={crc32}, patch={entry['patch']}, "
                  f"date={entry['date']}")

            # Insert the file entry
            cursor.execute(INSERT_FILE_QUERY, (dist, entry["patch"], entry["date"], fileid))

    # Commit the files
    connection.commit()
