# LPPC

Stands for "**L**east **P**rivilege **P**olicy **C**reator".
In AWS Terraform configurations we commonly use "deployer roles". Each Terraform root module gets its own deployer role which is only allowed to perform actions needed to CRUD (create, read, update, delete) the resources within that root module.
The CLI tool [lppc-cli](./lppc-cli) performs a static code analysis and suggests a policy based on pre-defined mappings. This allows to create least-privilege permissions before the first deployment.

The [mapping-creator](./mapping-creator) is the tool to conveniently create those mappings. The default mappings can be found at [https://github.com/bebold-jhr/lppc-aws-mappings](https://github.com/bebold-jhr/lppc-aws-mappings)

Both tools have been created using [claude code](https://code.claude.com/docs/en/overview).