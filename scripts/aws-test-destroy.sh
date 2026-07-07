#!/usr/bin/env bash
#
# OgreNote — AWS Budget Test Deployment Tear-Down
#
# Removes all resources created by aws-test-deploy.sh.
# Reverse order: ECS → ALB → IAM → ECR → Storage → VPC.
#
# Usage:
#   source scripts/aws-test-config.env
#   ./scripts/aws-test-destroy.sh

set -euo pipefail

# Disable the AWS CLI v2 pager so describe/query output doesn't open `less`
# for every call (which makes the script look hung and hides progress).
export AWS_PAGER=""

# ─── Validate config ───────────────────────────────────────────

for var in AWS_REGION STACK_PREFIX; do
    if [ -z "${!var:-}" ]; then
        echo "ERROR: $var is not set. Source your config file first."
        exit 1
    fi
done

PREFIX="${STACK_PREFIX}"
REGION="${AWS_REGION}"
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)

# Safety: refuse to destroy anything that looks like production
if echo "$PREFIX" | grep -iqE "^prod"; then
    echo "ERROR: Refusing to destroy resources with prefix '${PREFIX}' (looks like production)."
    echo "       This script is for test deployments only."
    exit 1
fi

TABLE_NAME="${PREFIX}ogrenote"
BUCKET_NAME="${PREFIX}ogrenote"
ECR_REPO="${PREFIX}ogrenote"
CLUSTER_NAME="${PREFIX}ogrenote"
SERVICE_NAME="${PREFIX}ogrenote-api"
TASK_FAMILY="${PREFIX}ogrenote-api"
LOG_GROUP="/ecs/${PREFIX}ogrenote"
ALB_NAME="${PREFIX}ogrenote-alb"
TG_NAME="${PREFIX}ogrenote-tg"
VPC_NAME="${PREFIX}ogrenote-vpc"
EXEC_ROLE_NAME="${PREFIX}ogrenote-exec"
TASK_ROLE_NAME="${PREFIX}ogrenote-task"
BUDGET_NAME="${PREFIX}ogrenote-budget"
# Phase 6 M-6.4 piece D: async-worker service teardown names.
WORKER_SERVICE_NAME="${PREFIX}ogrenote-worker"
WORKER_TASK_FAMILY="${PREFIX}ogrenote-worker"
WORKER_LOG_GROUP="/ecs/${PREFIX}ogrenote-worker"
WORKER_SCALING_POLICY="${PREFIX}ogrenote-worker-cpu"
WORKER_SCALABLE_RESOURCE="service/${PREFIX}ogrenote/${PREFIX}ogrenote-worker"

echo "=== OgreNote Test Tear-Down ==="
echo "Region:  ${REGION}"
echo "Prefix:  ${PREFIX}"
echo ""
echo "This will PERMANENTLY DELETE all resources with prefix '${PREFIX}'."
echo ""
read -p "Type 'destroy' to confirm: " CONFIRM
if [ "$CONFIRM" != "destroy" ]; then
    echo "Aborted."
    exit 0
fi

echo ""

# ─── ECS ────────────────────────────────────────────────────────

echo "--- Removing ECS ---"

# Scale service to 0 and delete
aws ecs update-service --cluster "$CLUSTER_NAME" --service "$SERVICE_NAME" \
    --desired-count 0 --region "${REGION}" 2>/dev/null && \
    echo "Scaled service to 0" || true

aws ecs delete-service --cluster "$CLUSTER_NAME" --service "$SERVICE_NAME" \
    --force --region "${REGION}" 2>/dev/null && \
    echo "Deleted service" || true

# Worker service (Phase 6 M-6.4 piece D). Deregister the autoscaling
# target first so Application Auto Scaling stops trying to maintain a
# desired count while we delete the service. Then scale to 0 + delete,
# same as the API. Both drain during the shared wait below.
aws application-autoscaling delete-scaling-policy \
    --service-namespace ecs \
    --resource-id "$WORKER_SCALABLE_RESOURCE" \
    --scalable-dimension ecs:service:DesiredCount \
    --policy-name "$WORKER_SCALING_POLICY" \
    --region "${REGION}" 2>/dev/null && \
    echo "Deleted worker scaling policy" || true
aws application-autoscaling deregister-scalable-target \
    --service-namespace ecs \
    --resource-id "$WORKER_SCALABLE_RESOURCE" \
    --scalable-dimension ecs:service:DesiredCount \
    --region "${REGION}" 2>/dev/null && \
    echo "Deregistered worker scalable target" || true

aws ecs update-service --cluster "$CLUSTER_NAME" --service "$WORKER_SERVICE_NAME" \
    --desired-count 0 --region "${REGION}" 2>/dev/null && \
    echo "Scaled worker service to 0" || true
aws ecs delete-service --cluster "$CLUSTER_NAME" --service "$WORKER_SERVICE_NAME" \
    --force --region "${REGION}" 2>/dev/null && \
    echo "Deleted worker service" || true

# Brief wait for the service to start draining. The deploy script now
# handles INACTIVE clusters on re-creation, so we don't need to wait
# for full drain — just give ECS a moment to begin cleanup.
echo "Waiting for service to drain (30s)..."
sleep 30

# Deregister task definitions
TASK_DEFS=$(aws ecs list-task-definitions --family-prefix "$TASK_FAMILY" \
    --query 'taskDefinitionArns' --output text --region "${REGION}" 2>/dev/null || true)
for td in $TASK_DEFS; do
    aws ecs deregister-task-definition --task-definition "$td" --region "${REGION}" 2>/dev/null || true
done
[ -n "$TASK_DEFS" ] && echo "Deregistered task definitions" || true

# Worker task definitions (Phase 6 M-6.4 piece D)
WORKER_TASK_DEFS=$(aws ecs list-task-definitions --family-prefix "$WORKER_TASK_FAMILY" \
    --query 'taskDefinitionArns' --output text --region "${REGION}" 2>/dev/null || true)
for td in $WORKER_TASK_DEFS; do
    aws ecs deregister-task-definition --task-definition "$td" --region "${REGION}" 2>/dev/null || true
done
[ -n "$WORKER_TASK_DEFS" ] && echo "Deregistered worker task definitions" || true

# Delete cluster (retry once if it fails due to lingering resources)
aws ecs delete-cluster --cluster "$CLUSTER_NAME" --region "${REGION}" 2>/dev/null && \
    echo "Deleted cluster" || {
        echo "Cluster delete failed, retrying in 10s..."
        sleep 10
        aws ecs delete-cluster --cluster "$CLUSTER_NAME" --region "${REGION}" 2>/dev/null && \
            echo "Deleted cluster (retry)" || echo "Cluster may need manual deletion"
    }

# Log group
aws logs delete-log-group --log-group-name "$LOG_GROUP" --region "${REGION}" 2>/dev/null && \
    echo "Deleted log group" || true
aws logs delete-log-group --log-group-name "$WORKER_LOG_GROUP" --region "${REGION}" 2>/dev/null && \
    echo "Deleted worker log group" || true

echo ""

# ─── ElastiCache Redis ──────────────────────────────────────────

echo "--- Removing Redis ---"

REDIS_CLUSTER="${PREFIX}redis"
aws elasticache delete-cache-cluster --cache-cluster-id "$REDIS_CLUSTER" \
    --region "${REGION}" 2>/dev/null && \
    echo "Deleting Redis cluster: $REDIS_CLUSTER (takes ~5 min)" || echo "No Redis cluster found"

# Wait for Redis to finish deleting before removing the subnet group
if aws elasticache describe-cache-clusters --cache-cluster-id "$REDIS_CLUSTER" \
    --region "${REGION}" 2>/dev/null | grep -q "deleting"; then
    echo "Waiting for Redis cluster to delete..."
    aws elasticache wait cache-cluster-deleted --cache-cluster-id "$REDIS_CLUSTER" \
        --region "${REGION}" 2>/dev/null || true
fi

# Delete cache subnet group
aws elasticache delete-cache-subnet-group \
    --cache-subnet-group-name "${PREFIX}redis-subnets" \
    --region "${REGION}" 2>/dev/null && \
    echo "Deleted cache subnet group" || true

# Delete Redis security group
REDIS_SG=$(aws ec2 describe-security-groups \
    --filters "Name=tag:Name,Values=${PREFIX}redis-sg" \
    --query 'SecurityGroups[0].GroupId' --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$REDIS_SG" != "None" ] && [ -n "$REDIS_SG" ]; then
    aws ec2 delete-security-group --group-id "$REDIS_SG" --region "${REGION}" 2>/dev/null && \
        echo "Deleted Redis security group" || true
fi

echo ""

# ─── ALB ────────────────────────────────────────────────────────

echo "--- Removing ALB ---"

ALB_ARN=$(aws elbv2 describe-load-balancers --names "$ALB_NAME" \
    --query 'LoadBalancers[0].LoadBalancerArn' --output text --region "${REGION}" 2>/dev/null || echo "NONE")

if [ "$ALB_ARN" != "NONE" ] && [ -n "$ALB_ARN" ]; then
    # Delete listeners
    LISTENERS=$(aws elbv2 describe-listeners --load-balancer-arn "$ALB_ARN" \
        --query 'Listeners[].ListenerArn' --output text --region "${REGION}" 2>/dev/null || true)
    for l in $LISTENERS; do
        aws elbv2 delete-listener --listener-arn "$l" --region "${REGION}" 2>/dev/null || true
    done

    aws elbv2 delete-load-balancer --load-balancer-arn "$ALB_ARN" --region "${REGION}" && \
        echo "Deleted ALB — waiting for full deletion..." || true
    # Wait for ALB to fully delete before removing the target group.
    # Without this, TG deletion fails because the ALB still references it.
    aws elbv2 wait load-balancers-deleted --load-balancer-arns "$ALB_ARN" \
        --region "${REGION}" 2>/dev/null || true
fi

TG_ARN=$(aws elbv2 describe-target-groups --names "$TG_NAME" \
    --query 'TargetGroups[0].TargetGroupArn' --output text --region "${REGION}" 2>/dev/null || echo "NONE")
if [ "$TG_ARN" != "NONE" ] && [ -n "$TG_ARN" ]; then
    aws elbv2 delete-target-group --target-group-arn "$TG_ARN" --region "${REGION}" 2>/dev/null && \
        echo "Deleted target group" || echo "Target group deletion failed — retry manually"
fi

echo ""

# ─── IAM ────────────────────────────────────────────────────────

echo "--- Removing IAM ---"

# Task role
aws iam delete-role-policy --role-name "$TASK_ROLE_NAME" \
    --policy-name "ogrenote-task-policy" 2>/dev/null || true
aws iam delete-role --role-name "$TASK_ROLE_NAME" 2>/dev/null && \
    echo "Deleted task role" || true

# Execution role
aws iam detach-role-policy --role-name "$EXEC_ROLE_NAME" \
    --policy-arn "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy" 2>/dev/null || true
aws iam delete-role --role-name "$EXEC_ROLE_NAME" 2>/dev/null && \
    echo "Deleted execution role" || true

echo ""

# ─── ECR ────────────────────────────────────────────────────────

echo "--- Removing ECR ---"

aws ecr delete-repository --repository-name "$ECR_REPO" --force --region "${REGION}" 2>/dev/null && \
    echo "Deleted ECR repo (including all images)" || true

echo ""

# ─── Storage ────────────────────────────────────────────────────

echo "--- Removing Storage ---"

# DynamoDB
aws dynamodb delete-table --table-name "$TABLE_NAME" --region "${REGION}" 2>/dev/null && \
    echo "Deleted DynamoDB table: $TABLE_NAME" || true

# S3 — must empty bucket first (handles versioned objects too)
echo "Emptying S3 bucket: $BUCKET_NAME"
aws s3 rm "s3://${BUCKET_NAME}" --recursive --region "${REGION}" 2>/dev/null || true
# Also purge versioned objects and delete markers if versioning was ever enabled
aws s3api list-object-versions --bucket "$BUCKET_NAME" --region "${REGION}" \
    --query '{Objects: Versions[].{Key:Key,VersionId:VersionId}}' --output json 2>/dev/null | \
    aws s3api delete-objects --bucket "$BUCKET_NAME" --delete file:///dev/stdin \
    --region "${REGION}" 2>/dev/null || true
aws s3api list-object-versions --bucket "$BUCKET_NAME" --region "${REGION}" \
    --query '{Objects: DeleteMarkers[].{Key:Key,VersionId:VersionId}}' --output json 2>/dev/null | \
    aws s3api delete-objects --bucket "$BUCKET_NAME" --delete file:///dev/stdin \
    --region "${REGION}" 2>/dev/null || true
aws s3api delete-bucket --bucket "$BUCKET_NAME" --region "${REGION}" 2>/dev/null && \
    echo "Deleted S3 bucket" || true

echo ""

# ─── VPC ────────────────────────────────────────────────────────

echo "--- Removing VPC ---"

VPC_ID=$(aws ec2 describe-vpcs \
    --filters "Name=tag:Name,Values=${VPC_NAME}" \
    --query 'Vpcs[0].VpcId' --output text --region "${REGION}" 2>/dev/null || echo "None")

if [ "$VPC_ID" != "None" ] && [ -n "$VPC_ID" ]; then
    # Delete VPC endpoints
    VPCE_IDS=$(aws ec2 describe-vpc-endpoints \
        --filters "Name=vpc-id,Values=${VPC_ID}" \
        --query 'VpcEndpoints[].VpcEndpointId' --output text --region "${REGION}" 2>/dev/null || true)
    for vpce in $VPCE_IDS; do
        aws ec2 delete-vpc-endpoints --vpc-endpoint-ids "$vpce" --region "${REGION}" 2>/dev/null || true
    done

    # Delete security groups (non-default)
    for sg_name in "${PREFIX}alb-sg" "${PREFIX}ecs-sg" "${PREFIX}redis-sg"; do
        SG_ID=$(aws ec2 describe-security-groups \
            --filters "Name=tag:Name,Values=${sg_name}" "Name=vpc-id,Values=${VPC_ID}" \
            --query 'SecurityGroups[0].GroupId' --output text --region "${REGION}" 2>/dev/null || echo "None")
        if [ "$SG_ID" != "None" ] && [ -n "$SG_ID" ]; then
            aws ec2 delete-security-group --group-id "$SG_ID" --region "${REGION}" 2>/dev/null || true
        fi
    done

    # Delete subnets
    SUBNET_IDS=$(aws ec2 describe-subnets \
        --filters "Name=vpc-id,Values=${VPC_ID}" \
        --query 'Subnets[].SubnetId' --output text --region "${REGION}" 2>/dev/null || true)
    for sub in $SUBNET_IDS; do
        aws ec2 delete-subnet --subnet-id "$sub" --region "${REGION}" 2>/dev/null || true
    done

    # Delete route tables (non-main)
    RTB_IDS=$(aws ec2 describe-route-tables \
        --filters "Name=vpc-id,Values=${VPC_ID}" "Name=tag:Name,Values=${PREFIX}*" \
        --query 'RouteTables[].RouteTableId' --output text --region "${REGION}" 2>/dev/null || true)
    for rtb in $RTB_IDS; do
        # Disassociate first
        ASSOC_IDS=$(aws ec2 describe-route-tables --route-table-ids "$rtb" \
            --query 'RouteTables[0].Associations[?!Main].RouteTableAssociationId' \
            --output text --region "${REGION}" 2>/dev/null || true)
        for assoc in $ASSOC_IDS; do
            aws ec2 disassociate-route-table --association-id "$assoc" --region "${REGION}" 2>/dev/null || true
        done
        aws ec2 delete-route-table --route-table-id "$rtb" --region "${REGION}" 2>/dev/null || true
    done

    # Detach and delete internet gateway
    IGW_IDS=$(aws ec2 describe-internet-gateways \
        --filters "Name=attachment.vpc-id,Values=${VPC_ID}" \
        --query 'InternetGateways[].InternetGatewayId' --output text --region "${REGION}" 2>/dev/null || true)
    for igw in $IGW_IDS; do
        aws ec2 detach-internet-gateway --internet-gateway-id "$igw" --vpc-id "$VPC_ID" --region "${REGION}" 2>/dev/null || true
        aws ec2 delete-internet-gateway --internet-gateway-id "$igw" --region "${REGION}" 2>/dev/null || true
    done

    # Delete VPC
    aws ec2 delete-vpc --vpc-id "$VPC_ID" --region "${REGION}" 2>/dev/null && \
        echo "Deleted VPC: $VPC_ID" || echo "VPC deletion failed — may have lingering ENIs from ALB (retry in 60s)"
fi

echo ""

# ─── DNS + HTTPS ────────────────────────────────────────────────

DOMAIN_NAME="${DOMAIN_NAME:-}"
if [ -n "$DOMAIN_NAME" ]; then
    echo "--- Removing DNS + HTTPS ---"

    # Find the hosted zone (may be a parent domain, e.g., example.com for ogrenotes.example.com)
    ZONE_ID=""
    LOOKUP_DOMAIN="${DOMAIN_NAME}"
    while [ -n "$LOOKUP_DOMAIN" ]; do
        ZONE_ID=$(aws route53 list-hosted-zones-by-name --dns-name "${LOOKUP_DOMAIN}." \
            --query "HostedZones[?Name=='${LOOKUP_DOMAIN}.'].Id" --output text 2>/dev/null | head -1 | sed 's|/hostedzone/||')
        if [ -n "$ZONE_ID" ] && [ "$ZONE_ID" != "None" ]; then
            break
        fi
        LOOKUP_DOMAIN="${LOOKUP_DOMAIN#*.}"
        if ! echo "$LOOKUP_DOMAIN" | grep -q '\.'; then
            ZONE_ID=""
            break
        fi
    done

    if [ -n "$ZONE_ID" ] && [ "$ZONE_ID" != "None" ]; then
        # Delete A record (alias to ALB)
        # Need the ALB DNS and hosted zone ID to construct the delete request
        ALB_DNS_FOR_DELETE=$(aws route53 list-resource-record-sets --hosted-zone-id "$ZONE_ID" \
            --query "ResourceRecordSets[?Name=='${DOMAIN_NAME}.' && Type=='A'].AliasTarget.DNSName" \
            --output text 2>/dev/null || true)
        ALB_HZ_FOR_DELETE=$(aws route53 list-resource-record-sets --hosted-zone-id "$ZONE_ID" \
            --query "ResourceRecordSets[?Name=='${DOMAIN_NAME}.' && Type=='A'].AliasTarget.HostedZoneId" \
            --output text 2>/dev/null || true)

        if [ -n "$ALB_DNS_FOR_DELETE" ] && [ "$ALB_DNS_FOR_DELETE" != "None" ]; then
            aws route53 change-resource-record-sets --hosted-zone-id "$ZONE_ID" \
                --change-batch "{
                    \"Changes\": [{
                        \"Action\": \"DELETE\",
                        \"ResourceRecordSet\": {
                            \"Name\": \"${DOMAIN_NAME}\",
                            \"Type\": \"A\",
                            \"AliasTarget\": {
                                \"HostedZoneId\": \"${ALB_HZ_FOR_DELETE}\",
                                \"DNSName\": \"${ALB_DNS_FOR_DELETE}\",
                                \"EvaluateTargetHealth\": false
                            }
                        }
                    }]
                }" 2>/dev/null && echo "Deleted DNS A record for ${DOMAIN_NAME}" || true
        fi

        # Delete ACM validation CNAME records created by the deploy script
        # These are the _acm-challenge.domain.com CNAME records
        VALIDATION_RECORDS=$(aws route53 list-resource-record-sets --hosted-zone-id "$ZONE_ID" \
            --query "ResourceRecordSets[?starts_with(Name, '_') && Type=='CNAME'].[Name,ResourceRecords[0].Value]" \
            --output text 2>/dev/null || true)
        while IFS=$'\t' read -r vname vvalue; do
            [ -z "$vname" ] && continue
            aws route53 change-resource-record-sets --hosted-zone-id "$ZONE_ID" \
                --change-batch "{
                    \"Changes\": [{
                        \"Action\": \"DELETE\",
                        \"ResourceRecordSet\": {
                            \"Name\": \"${vname}\",
                            \"Type\": \"CNAME\",
                            \"TTL\": 300,
                            \"ResourceRecords\": [{\"Value\": \"${vvalue}\"}]
                        }
                    }]
                }" 2>/dev/null || true
        done <<< "$VALIDATION_RECORDS"
        echo "Cleaned up validation CNAME records"
    fi

    # Delete ACM certificate
    CERT_ARN=$(aws acm list-certificates \
        --query "CertificateSummaryList[?DomainName=='${DOMAIN_NAME}'].CertificateArn" \
        --output text --region "${REGION}" 2>/dev/null | head -1)
    if [ -n "$CERT_ARN" ] && [ "$CERT_ARN" != "None" ]; then
        aws acm delete-certificate --certificate-arn "$CERT_ARN" --region "${REGION}" 2>/dev/null && \
            echo "Deleted ACM certificate" || echo "ACM cert deletion failed — may still be in use by ALB (retry after ALB is fully deleted)"
    fi

    # Note: we do NOT delete the Route 53 hosted zone itself — it may be shared
    # with other services or have NS records at the registrar.
    echo "NOTE: Route 53 hosted zone for ${DOMAIN_NAME} was NOT deleted (may be shared)."

    echo ""
fi

# ─── Budget ─────────────────────────────────────────────────────

echo "--- Removing Budget ---"
aws budgets delete-budget --account-id "$ACCOUNT_ID" \
    --budget-name "$BUDGET_NAME" 2>/dev/null && \
    echo "Deleted budget alarm" || true

echo ""
echo "==========================================="
echo "  Tear-down complete."
echo "==========================================="
echo ""
echo "  Verify no orphaned resources in the AWS console:"
echo "    - EC2 > Load Balancers"
echo "    - ECS > Clusters"
echo "    - VPC > Your VPCs"
echo "    - DynamoDB > Tables"
echo "    - S3 > Buckets"
echo "    - ACM > Certificates"
echo "    - Route 53 > Hosted Zones (records, not the zone itself)"
echo ""
echo "  Some resources (ALB, ENIs) may take a few minutes to fully delete."
echo "  If VPC deletion failed, wait 60 seconds and run this script again."
