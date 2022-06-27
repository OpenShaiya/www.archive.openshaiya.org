# OpenShaiya - Public Archive

This website serves as a simple s3 bucket listing, which hosts the contents
of the Shaiya archive.

## Downloading a copy of the archive

As the OpenShaiya archive is hosted on AWS S3, running the following command will download a copy of the entire archive to local disk. Please note that
this archive is approximiately 150gb, so make sure you have plenty of disk space and available bandwidth:
`aws s3 sync s3://archive.openshaiya.org . --region us-east-1 --no-sign-request`

[aws-cli]: https://aws.amazon.com/cli/
